use alloy_consensus::TxEip1559;
use alloy_eips::eip2930::AccessList;
use alloy_network::TxSigner;
use alloy_primitives::{address, Address, Bytes, ChainId, U256};
use alloy_rlp::Encodable;
use alloy_signer_local::coins_bip39::English;
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::SolValue;
use bon::builder;
use op_alloy_consensus::OpTypedTransaction;
use reth::chainspec::MAINNET;
use reth::transaction_pool::blobstore::InMemoryBlobStore;
use reth::transaction_pool::validate::EthTransactionValidatorBuilder;
use reth_optimism_node::txpool::{OpPooledTransaction, OpTransactionValidator};
use reth_optimism_primitives::OpTransactionSigned;
use reth_primitives::transaction::SignedTransactionIntoRecoveredExt;
use revm_primitives::TxKind;
use semaphore::identity::Identity;
use semaphore::poseidon_tree::LazyPoseidonTree;
use semaphore::{hash_to_field, Field};

use world_chain_builder_pbh::external_nullifier::ExternalNullifier;
use world_chain_builder_pbh::payload::{PbhPayload, Proof, TREE_DEPTH};

use crate::bindings::IEntryPoint::{self, PackedUserOperation, UserOpsPerAggregator};
use crate::bindings::IMulticall3;
use crate::bindings::IPBHEntryPoint::{self};
use crate::mock::MockEthProvider;
use crate::root::WorldChainRootValidator;
use crate::tx::WorldChainPooledTransaction;
use crate::validator::WorldChainTransactionValidator;

const MNEMONIC: &str = "test test test test test test test test test test test junk";

pub fn signer(index: u32) -> PrivateKeySigner {
    let signer = alloy_signer_local::MnemonicBuilder::<English>::default()
        .phrase(MNEMONIC)
        .index(index)
        .expect("Failed to set index")
        .build()
        .expect("Failed to create signer");

    signer
}

#[cfg(test)]
#[test]
fn test_signer() {
    let signer = signer(0);

    println!("Signer: {:?}", signer);
}

pub fn account(index: u32) -> Address {
    let signer = signer(index);
    signer.address()
}

pub fn identity(index: u32) -> Identity {
    let mut secret = account(index).into_word().0;

    Identity::from_secret(&mut secret as &mut _, None)
}

// TODO: Cache with Once or lazy-static?
pub fn tree() -> LazyPoseidonTree {
    let mut tree = LazyPoseidonTree::new(TREE_DEPTH, Field::ZERO);

    // Only accounts 0 through 5 are included in the tree
    for acc in 0..=5 {
        let identity = identity(acc);
        let commitment = identity.commitment();

        tree = tree.update_with_mutation(acc as usize, &commitment);
    }

    tree.derived()
}

pub fn tree_root() -> Field {
    tree().root()
}

pub fn tree_inclusion_proof(acc: u32) -> semaphore::poseidon_tree::Proof {
    tree().proof(acc as usize)
}

pub fn nullifier_hash(acc: u32, external_nullifier: Field) -> Field {
    let identity = identity(acc);

    semaphore::protocol::generate_nullifier_hash(&identity, external_nullifier)
}

pub fn semaphore_proof(
    acc: u32,
    ext_nullifier: Field,
    signal: Field,
) -> semaphore::protocol::Proof {
    let identity = identity(acc);
    let incl_proof = tree_inclusion_proof(acc);

    semaphore::protocol::generate_proof(&identity, &incl_proof, ext_nullifier, signal)
        .expect("Failed to generate semaphore proof")
}

#[builder]
pub fn eip1559(
    #[builder(default = 1)] chain_id: ChainId,
    #[builder(default = 0)] nonce: u64,
    #[builder(default = 150000)] gas_limit: u64,
    #[builder(default = 10000000)] // 0.1 GWEI
    max_fee_per_gas: u128,
    #[builder(default = 0)] max_priority_fee_per_gas: u128,
    #[builder(into)] to: TxKind,
    #[builder(default = U256::ZERO)] value: U256,
    #[builder(default)] access_list: AccessList,
    #[builder(into, default)] input: Bytes,
) -> TxEip1559 {
    TxEip1559 {
        chain_id,
        nonce,
        gas_limit,
        max_fee_per_gas,
        max_priority_fee_per_gas,
        to,
        value,
        access_list,
        input,
    }
}

pub async fn eth_tx(acc: u32, mut tx: TxEip1559) -> OpPooledTransaction {
    let signer = signer(acc);
    let signature = signer
        .sign_transaction(&mut tx)
        .await
        .expect("Failed to sign transaction");
    let op_tx: OpTypedTransaction = tx.into();
    let tx_signed = OpTransactionSigned::new(op_tx, signature);
    let pooled = OpPooledTransaction::new(
        tx_signed.clone().into_ecrecovered_unchecked().unwrap(),
        tx_signed.eip1559().unwrap().size(),
    );
    pooled
}

#[builder]
pub fn user_op(
    acc: u32,
    #[builder(into, default = U256::ZERO)] nonce: U256,
    #[builder(default = ExternalNullifier::v1(12, 2024, 0))] external_nullifier: ExternalNullifier,
) -> (IEntryPoint::PackedUserOperation, PbhPayload) {
    let sender = account(acc);

    let user_op = PackedUserOperation {
        sender,
        nonce,
        ..Default::default()
    };

    let signal = crate::eip4337::hash_user_op(&user_op);

    let tree = tree();
    let root = tree.root();
    let proof = semaphore_proof(acc, external_nullifier.to_word(), signal);
    let nullifier_hash = nullifier_hash(acc, external_nullifier.to_word());

    let proof = Proof(proof);

    let payload = PbhPayload {
        external_nullifier,
        nullifier_hash,
        root,
        proof,
    };

    (user_op, payload)
}

pub fn pbh_bundle(
    user_ops: Vec<PackedUserOperation>,
    proofs: Vec<PbhPayload>,
) -> IPBHEntryPoint::handleAggregatedOpsCall {
    let mut signature_buff = Vec::new();
    proofs.encode(&mut signature_buff);

    IPBHEntryPoint::handleAggregatedOpsCall {
        _0: vec![UserOpsPerAggregator {
            userOps: user_ops,
            signature: signature_buff.into(),
            aggregator: PBH_TEST_SIGNATURE_AGGREGATOR,
        }],
        _1: PBH_TEST_ENTRYPOINT,
    }
}

#[builder]
pub fn pbh_multicall(
    acc: u32,
    #[builder(default = ExternalNullifier::v1(12, 2024, 0))] external_nullifier: ExternalNullifier,
) -> IPBHEntryPoint::pbhMulticallCall {
    let sender = account(acc);

    let call = IMulticall3::Call3::default();
    let calls = vec![call];

    let signal_hash: alloy_primitives::Uint<256, 4> =
        hash_to_field(&SolValue::abi_encode_packed(&(sender, calls.clone())));

    let tree = tree();
    let root = tree.root();
    let proof = semaphore_proof(acc, external_nullifier.to_word(), signal_hash);
    let nullifier_hash = nullifier_hash(acc, external_nullifier.to_word());

    let proof = [
        proof.0 .0,
        proof.0 .1,
        proof.1 .0[0],
        proof.1 .0[1],
        proof.1 .1[0],
        proof.1 .1[1],
        proof.2 .0,
        proof.2 .1,
    ];

    // TODO: Switch to ruint in semaphore-rs and remove this
    let proof: [U256; 8] = proof
        .into_iter()
        .map(|x| {
            let mut bytes_repr: [u8; 32] = [0; 32];
            x.to_big_endian(&mut bytes_repr);

            U256::from_be_bytes(bytes_repr)
        })
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();

    let payload = IPBHEntryPoint::PBHPayload {
        root,
        pbhExternalNullifier: external_nullifier.to_word(),
        nullifierHash: nullifier_hash,
        proof,
    };

    IPBHEntryPoint::pbhMulticallCall { calls, payload }
}

pub const PBH_TEST_SIGNATURE_AGGREGATOR: Address =
    address!("09635F643e140090A9A8Dcd712eD6285858ceBef");

pub const PBH_TEST_ENTRYPOINT: Address = address!("7a2088a1bFc9d81c55368AE168C2C02570cB814F");

pub const TEST_WORLD_ID: Address = address!("047eE5313F98E26Cc8177fA38877cB36292D2364");

pub fn world_chain_validator(
) -> WorldChainTransactionValidator<MockEthProvider, WorldChainPooledTransaction> {
    let client = MockEthProvider::default();
    let validator = EthTransactionValidatorBuilder::new(MAINNET.clone())
        .no_shanghai()
        .no_cancun()
        .build(client.clone(), InMemoryBlobStore::default());
    let validator = OpTransactionValidator::new(validator).require_l1_data_gas_fee(false);
    let root_validator = WorldChainRootValidator::new(client, TEST_WORLD_ID).unwrap();
    WorldChainTransactionValidator::new(
        validator,
        root_validator,
        30,
        PBH_TEST_ENTRYPOINT,
        PBH_TEST_SIGNATURE_AGGREGATOR,
    )
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(0, "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266")]
    #[test_case(1, "0x70997970C51812dc3A010C7d01b50e0d17dc79C8")]
    #[test_case(2, "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC")]
    #[test_case(3, "0x90F79bf6EB2c4f870365E785982E1f101E93b906")]
    #[test_case(4, "0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65")]
    #[test_case(5, "0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc")]
    #[test_case(6, "0x976EA74026E726554dB657fA54763abd0C3a0aa9")]
    #[test_case(7, "0x14dC79964da2C08b23698B3D3cc7Ca32193d9955")]
    #[test_case(8, "0x23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f")]
    #[test_case(9, "0xa0Ee7A142d267C1f36714E4a8F75612F20a79720")]
    fn mnemonic_accounts(index: u32, exp_address: &str) {
        let exp: Address = exp_address.parse().unwrap();

        assert_eq!(exp, account(index));
    }

    #[test]
    fn treeroot() {
        println!("Tree Root: {:?}", tree_root());
    }
}
