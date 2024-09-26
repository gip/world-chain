use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use semaphore::packed_proof::PackedProof;
use semaphore::Field;
use serde::{Deserialize, Serialize};

const LEN: usize = 256;

pub type ProofBytes = [u8; LEN];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof(pub semaphore::protocol::Proof);

impl Default for Proof {
    fn default() -> Self {
        let proof = semaphore::protocol::Proof(
            (0u64.into(), 0u64.into()),
            ([0u64.into(), 0u64.into()], [0u64.into(), 0u64.into()]),
            (0u64.into(), 0u64.into()),
        );

        Proof(proof)
    }
}

impl Decodable for Proof {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let bytes = ProofBytes::decode(buf)?;
        Ok(Proof(PackedProof(bytes).into()))
    }
}

impl Encodable for Proof {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let PackedProof(bytes) = self.0.into();
        bytes.encode(out)
    }

    fn length(&self) -> usize {
        LEN + 3
    }
}

#[derive(Clone, Debug, RlpEncodable, RlpDecodable, PartialEq, Eq, Default)]
pub struct SemaphoreProof {
    pub external_nullifier: String,
    pub external_nullifier_hash: Field,
    pub nullifier_hash: Field,
    pub signal_hash: Field,
    pub root: Field,
    pub proof: Proof,
}

#[cfg(test)]
mod test {

    use super::*;
    use ethers_core::types::U256;

    #[test]
    fn encode_decode() {
        let proof = Proof(semaphore::protocol::Proof(
            (U256::from(1u64), U256::from(2u64)),
            (
                [U256::from(3u64), U256::from(4u64)],
                [U256::from(5u64), U256::from(6u64)],
            ),
            (U256::from(7u64), U256::from(8u64)),
        ));
        let semaphore_proof = SemaphoreProof {
            external_nullifier: "0-012025-11".to_string(),
            external_nullifier_hash: Field::from(9u64),
            nullifier_hash: Field::from(10u64),
            signal_hash: Field::from(11u64),
            root: Field::from(12u64),
            proof,
        };
        let encoded = alloy_rlp::encode(&semaphore_proof);
        let mut buf = encoded.as_slice();
        let decoded = SemaphoreProof::decode(&mut buf).unwrap();
        assert_eq!(semaphore_proof, decoded);
    }
}
