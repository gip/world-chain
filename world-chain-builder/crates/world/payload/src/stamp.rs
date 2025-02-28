use alloy::sol;
use alloy_network::{EthereumWallet, NetworkWallet, TransactionBuilder};
use alloy_signer_local::{coins_bip39::English, MnemonicBuilder};
use eyre::eyre::eyre;
use futures::executor::block_on;
use op_alloy_consensus::OpTxEnvelope;
use op_alloy_network::Optimism;
use op_alloy_rpc_types::OpTransactionRequest;
use reth_optimism_node::OpEvm;
use std::sync::LazyLock;
use WorldChainBlockRegistry::stampBlockCall;

use crate::inspector::PBHCallTracer;

static BUILDER_MNEMONIC: LazyLock<String> =
    LazyLock::new(|| std::env::var("BUILDER_MNEMONIC").expect("BUILDER_MNEMONIC env var not set"));

sol! {
    #[sol(rpc)]
    interface WorldChainBlockRegistry {
        function stampBlock();
    }
}

pub fn stamp_block_tx<DB>(
    evm: &mut OpEvm<'_, &mut PBHCallTracer, &mut DB>,
) -> eyre::Result<(revm_primitives::Address, OpTxEnvelope)>
where
    DB: revm::Database + revm::DatabaseCommit,
    <DB as revm::Database>::Error: std::fmt::Debug + Send + Sync + derive_more::Error + 'static,
{
    let signer = MnemonicBuilder::<English>::default()
        .phrase(BUILDER_MNEMONIC.to_string())
        .index(1)?
        .build()?;

    let wallet = EthereumWallet::from(signer);
    let address = NetworkWallet::<Optimism>::default_signer_address(&wallet);
    let db = evm.db_mut();
    let nonce = db.basic(address)?.unwrap_or_default().nonce;

    // spawn a new os thread
    let tx = std::thread::spawn(move || {
        block_on(async {
            OpTransactionRequest::default()
                .nonce(nonce)
                .gas_limit(100000)
                .max_priority_fee_per_gas(100_000_000)
                .max_fee_per_gas(100_000_000)
                .with_chain_id(4801)
                .with_call(&stampBlockCall {})
                .build(&wallet)
                .await
        })
    })
    .join()
    .map_err(|e| eyre!("{e:?}"))?
    .map_err(|e| eyre!("{e:?}"))?;

    Ok((address, tx))
}
