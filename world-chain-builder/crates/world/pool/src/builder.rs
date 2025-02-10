use std::sync::Arc;

use alloy_primitives::Address;
use reth::builder::components::{PoolBuilder, PoolBuilderConfigOverrides};
use reth::builder::{BuilderContext, FullNodeTypes, NodeTypes};
use reth::transaction_pool::blobstore::DiskFileBlobStore;
use reth::transaction_pool::TransactionValidationTaskExecutor;
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_forks::OpHardforks;
use reth_optimism_node::txpool::OpTransactionValidator;
use reth_optimism_primitives::OpPrimitives;
use reth_provider::CanonStateSubscriptions;
use tracing::{debug, info};

use super::WorldChainTransactionPool;
use crate::ordering::WorldChainOrdering;
use crate::root::WorldChainRootValidator;
use crate::validator::WorldChainTransactionValidator;

/// A basic World Chain transaction pool.
///
/// This contains various settings that can be configured and take precedence over the node's
/// config.
#[derive(Debug, Clone)]
pub struct WorldChainPoolBuilder {
    pub num_pbh_txs: u8,
    pub pbh_entrypoint: Address,
    pub pbh_signature_aggregator: Address,
    pub world_id: Address,
    pub pool_config_overrides: PoolBuilderConfigOverrides,
}

impl WorldChainPoolBuilder {
    pub fn new(
        num_pbh_txs: u8,
        pbh_entrypoint: Address,
        pbh_signature_aggregator: Address,
        world_id: Address,
    ) -> Self {
        Self {
            num_pbh_txs,
            pbh_entrypoint,
            pbh_signature_aggregator,
            world_id,
            pool_config_overrides: PoolBuilderConfigOverrides::default(),
        }
    }

    pub fn with_pool_config_overrides(
        self,
        pool_config_overrides: PoolBuilderConfigOverrides,
    ) -> Self {
        Self {
            pool_config_overrides: pool_config_overrides,
            ..self
        }
    }
}

impl<Node> PoolBuilder<Node> for WorldChainPoolBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec: OpHardforks, Primitives = OpPrimitives>>,
{
    type Pool = WorldChainTransactionPool<Node::Provider, DiskFileBlobStore>;

    async fn build_pool(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        let Self {
            num_pbh_txs,
            pbh_entrypoint,
            pbh_signature_aggregator,
            world_id,
            pool_config_overrides,
            ..
        } = self;

        let data_dir = ctx.config().datadir();
        let blob_store = DiskFileBlobStore::open(data_dir.blobstore(), Default::default())?;

        let validator = TransactionValidationTaskExecutor::eth_builder(ctx.provider().clone())
            .no_eip4844()
            .with_head_timestamp(ctx.head().timestamp)
            .kzg_settings(ctx.kzg_settings()?)
            .with_additional_tasks(
                pool_config_overrides
                    .additional_validation_tasks
                    .unwrap_or_else(|| ctx.config().txpool.additional_validation_tasks),
            )
            .build_with_tasks(ctx.task_executor().clone(), blob_store.clone())
            .map(|validator| {
                let op_tx_validator = OpTransactionValidator::new(validator.clone())
                    // In --dev mode we can't require gas fees because we're unable to decode the L1
                    // block info
                    .require_l1_data_gas_fee(!ctx.config().dev.dev);
                let root_validator =
                    WorldChainRootValidator::new(validator.client().clone(), world_id)
                        .expect("failed to initialize root validator");
                WorldChainTransactionValidator::new(
                    op_tx_validator,
                    root_validator,
                    num_pbh_txs,
                    pbh_entrypoint,
                    pbh_signature_aggregator,
                )
            });

        let ordering = WorldChainOrdering::default();

        let transaction_pool =
            reth::transaction_pool::Pool::new(validator, ordering, blob_store, ctx.pool_config());
        info!(target: "reth::cli", "Transaction pool initialized");
        let transactions_path = data_dir.txpool_transactions();

        // spawn txpool maintenance task
        {
            let pool = transaction_pool.clone();
            let chain_events = ctx.provider().canonical_state_stream();
            let client = ctx.provider().clone();
            let transactions_backup_config =
                reth::transaction_pool::maintain::LocalTransactionBackupConfig::with_local_txs_backup(transactions_path);

            ctx.task_executor()
                .spawn_critical_with_graceful_shutdown_signal(
                    "local transactions backup task",
                    |shutdown| {
                        reth::transaction_pool::maintain::backup_local_transactions_task(
                            shutdown,
                            pool.clone(),
                            transactions_backup_config,
                        )
                    },
                );

            // spawn the maintenance task
            ctx.task_executor().spawn_critical(
                "txpool maintenance task",
                reth::transaction_pool::maintain::maintain_transaction_pool_future(
                    client,
                    pool,
                    chain_events,
                    ctx.task_executor().clone(),
                    Default::default(),
                ),
            );
            debug!(target: "reth::cli", "Spawned txpool maintenance task");
        }

        Ok(transaction_pool)
    }
}
