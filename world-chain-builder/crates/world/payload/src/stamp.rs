use alloy::sol;
use alloy_network::{EthereumWallet, NetworkWallet, TransactionBuilder};
use alloy_signer_local::PrivateKeySigner;
use eyre::eyre::eyre;
use futures::executor::block_on;
use op_alloy_consensus::OpTxEnvelope;
use op_alloy_network::Optimism;
use op_alloy_rpc_types::OpTransactionRequest;
use reth_optimism_node::OpEvm;
use WorldChainBlockRegistry::stampBlockCall;

use crate::{builder::WorldChainPayloadBuilderCtx, inspector::PBHCallTracer};

sol! {
    #[sol(rpc)]
    interface WorldChainBlockRegistry {
        function stampBlock();
    }
}

pub fn stamp_block_tx<DB, Client>(
    ctx: &WorldChainPayloadBuilderCtx<Client>,
    evm: &mut OpEvm<'_, &mut PBHCallTracer, &mut DB>,
) -> eyre::Result<(revm_primitives::Address, OpTxEnvelope)>
where
    DB: revm::Database + revm::DatabaseCommit,
    <DB as revm::Database>::Error: std::fmt::Debug + Send + Sync + derive_more::Error + 'static,
{
    let signer: PrivateKeySigner = ctx.builder_private_key.parse()?;
    let wallet = EthereumWallet::from(signer);
    let address = NetworkWallet::<Optimism>::default_signer_address(&wallet);
    let nonce = evm.db_mut().basic(address)?.unwrap_or_default().nonce;
    let base_fee: u128 = evm.context.evm.env.block.basefee.try_into().unwrap();
    let block_registry = ctx.block_registry;

    // spawn a new os thread
    let tx = std::thread::spawn(move || {
        block_on(async {
            OpTransactionRequest::default()
                .nonce(nonce)
                .gas_limit(100_000)
                .max_priority_fee_per_gas(base_fee)
                .max_fee_per_gas(base_fee)
                .with_chain_id(4801)
                .with_call(&stampBlockCall {})
                .to(block_registry)
                .build(&wallet)
                .await
        })
    })
    .join()
    .map_err(|e| eyre!("{e:?}"))?
    .map_err(|e| eyre!("{e:?}"))?;

    Ok((address, tx))
}
