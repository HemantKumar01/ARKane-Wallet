use crate::Error;
use bitcoin::taproot::ControlBlock;
use bitcoin::Amount;
use bitcoin::FeeRate;
use bitcoin::OutPoint;
use bitcoin::ScriptBuf;
use bitcoin::VarInt;

/// InputSize 41 bytes
///     - PreviousOutPoint:
///             - Hash: 32 bytes
///             - Index: 4 bytes
///     - OP_DATA: 1 byte (ScriptSigLength)
///     - ScriptSig: 0 bytes
///     - Witness <---- we use "Witness" instead of "ScriptSig" for transaction validation, but
///    "Witness" is stored separately and weight for it size is smaller. So we separate the
///    calculation of ordinary data from witness data.
///     - Sequence: 4 bytes
const INPUT_SIZE: usize = 32 + 4 + 1 + 4;

/// TaprootBaseControlBlockWitnessSize 33 bytes
///      - leafVersionAndParity: 1 byte
///      - schnorrPubKey: 32 byte
const TAPROOT_BASE_CONTROL_BLOCK_WITNESS_SIZE: usize = 33;

/// BaseTxSize 8 bytes
///      - Version: 4 bytes
///      - LockTime: 4 bytes
const BASE_TX_SIZE: usize = 8;

///  WitnessHeaderSize 2 bytes
///             - Flag: 1 byte
///             - Marker: 1 byte
const WITNESS_HEADER_SIZE: usize = 4;

///  WitnessScaleFactor determines the level of "discount" witness data
///      receives compared to "base" data. A scale factor of 4, denotes that
///      witness data is 1/4 as cheap as regular non-witness data.
const WITNESS_SCALE_FACTOR: usize = 4;

/// BASE_OUTPUT_SIZE 9 bytes
///     - value: 8 bytes
///     - var_int: 1 byte (pkscript_length)
const BASE_OUTPUT_SIZE: usize = 8 + 1;

/// P2PKHSIZE 25 bytes.
const P2PKH_SIZE: usize = 25;

/// P2TROutputSize 43 bytes
///      - value: 8 bytes
///      - var_int: 1 byte (pkscript_length)
///      - pkscript (p2tr): 34 bytes
const P2TR_OUTPUT_SIZE: usize = BASE_OUTPUT_SIZE + P2PKH_SIZE;

#[derive(Default)]
struct TxWeightEstimator {
    has_witness: bool,
    input_count: u32,
    output_count: u32,
    input_size: usize,
    input_witness_size: usize,
    output_size: usize,
}

impl TxWeightEstimator {
    /// Add an input with Tapscript details to the transaction.
    ///
    /// Updates the weight estimate to account for an additional
    /// input spending a segwit v1 pay-to-Taproot output using the script path. This
    /// accepts the total size of the witness for the script leaf that is executed
    /// and adds the size of the control block to the total witness size.
    ///
    /// NOTE: The leaf witness size must be calculated without the byte that accounts
    /// for the number of witness elements, only the total size of all elements on
    /// the stack that are consumed by the revealed script should be counted.
    pub fn add_tapscript_input(
        &mut self,
        leaf_witness_size: usize,
        revealed_script: &ScriptBuf,
        control_block: &ControlBlock,
    ) -> &mut Self {
        // We add 1 byte for the total number of witness elements.
        let control_block_witness_size = 1
            + TAPROOT_BASE_CONTROL_BLOCK_WITNESS_SIZE
            // 1 byte for the length of the element plus the element itself.
            + 1
            + revealed_script.len()
            + 1
            + control_block.size();

        self.input_size += INPUT_SIZE;
        self.input_witness_size += leaf_witness_size + control_block_witness_size;
        self.input_count += 1;
        self.has_witness = true;

        self
    }

    /// Updates the weight estimate to account for an additional native
    /// SegWit v1 P2TR output.
    pub fn add_p2tr_output(&mut self) -> &mut Self {
        self.output_size += P2TR_OUTPUT_SIZE;
        self.output_count += 1;
        self
    }

    /// Weight gets the estimated weight of the transaction.
    pub fn weight(&self) -> usize {
        let input_count_size = VarInt(self.input_count as u64).size();
        let output_count_size = VarInt(self.output_count as u64).size();

        let tx_size_stripped = BASE_TX_SIZE
            + input_count_size
            + self.input_size
            + output_count_size
            + self.output_size;

        let mut weight = tx_size_stripped * WITNESS_SCALE_FACTOR;

        if self.has_witness {
            weight += WITNESS_HEADER_SIZE + self.input_witness_size;
        }

        weight
    }

    /// VSize gets the estimated virtual size of the transactions, in vbytes.
    pub fn vsize(&self) -> usize {
        // A tx's vsize is 1/4 of the weight, rounded up.
        (self.weight() + 3) / 4
    }
}

pub struct VtxoInput {
    pub outpoint: OutPoint,
    pub amount: Amount,
    pub revealed_script: Option<ScriptBuf>,
    pub control_block: ControlBlock,
    pub witness_size: usize,
}

/// Compute the fee for a redeem transaction.
pub fn compute_redeem_tx_fee(
    fee_rate: FeeRate,
    vtxos: &[VtxoInput],
    num_outputs: usize,
) -> Result<Amount, Error> {
    if vtxos.is_empty() {
        return Err(Error::ad_hoc("missing VTXOs".to_string()));
    }

    let mut redeem_tx_estimator = TxWeightEstimator::default();

    // Estimate inputs.
    for vtxo in vtxos {
        if let Some(revealed_script) = &vtxo.revealed_script {
            redeem_tx_estimator.add_tapscript_input(
                vtxo.witness_size,
                revealed_script,
                &vtxo.control_block,
            );
        } else {
            return Err(Error::ad_hoc(format!(
                "missing tapscript for vtxo {}",
                vtxo.outpoint.txid
            )));
        }
    }

    // Estimate outputs.
    for _ in 0..num_outputs {
        redeem_tx_estimator.add_p2tr_output();
    }

    let vsize = redeem_tx_estimator.vsize();

    let fee = fee_rate
        .fee_vb(vsize as u64)
        .ok_or(Error::ad_hoc("failed calculating fee rate".to_string()))?;

    Ok(fee)
}
