use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use bitcoin::Txid;
use ark_core::ArkAddress;
use ark_core::ExplorerUtxo;
use bitcoin::Amount;

pub use ark_core::vtxo::VirtualTxOutpoints;
pub use ark_core::boarding_output::BoardingOutpoints;

#[derive(Clone)]
pub struct ArkAddressCli(pub ArkAddress);

impl std::str::FromStr for ArkAddressCli {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let address = ArkAddress::decode(s)?;
        Ok(Self(address))
    }
}

#[derive(Deserialize, Clone)]
pub struct Config {
    pub ark_server_url: String,
    pub esplora_url: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WalletInfo {
    pub id: String,
    pub seed: String,
}

pub struct AppState {
    pub wallets: Mutex<HashMap<String, WalletInfo>>,
    pub config: Config,
    pub server_info: Option<Mutex<ark_core::server::Info>>,
    pub esplora_client: Option<Mutex<EsploraClient>>,
}

#[derive(Serialize)]
pub struct AddressResponse {
    pub wallet_id: String,
    pub onchain_address: String,
    pub offchain_address: String,
}

#[derive(Serialize)]
pub struct WalletResponse {
    pub wallet_id: String,
}

#[derive(Serialize)]
pub struct BalanceResponse {
    pub wallet_id: String,
    pub offchain_balance: OffchainBalance,
    pub boarding_balance: BoardingBalance,
}

#[derive(Serialize)]
pub struct OffchainBalance {
    pub spendable: u64,
    pub expired: u64,
}

#[derive(Serialize)]
pub struct BoardingBalance {
    pub spendable: u64,
    pub expired: u64,
    pub pending: u64,
}

#[derive(Deserialize)]
pub struct SendToArkAddressRequest {
    pub wallet_id: String,
    pub address: String,
    pub amount: u64,
}

#[derive(Serialize)]
pub struct SendToArkAddressResponse {
    pub wallet_id: String,
    pub to_address: String,
    pub amount: u64,
    pub txid: String,
}

#[derive(Deserialize)]
pub struct FaucetRequest {
    pub onchain_address: String,
    pub amount: f64,
}

#[derive(Serialize)]
pub struct FaucetResponse {
    pub success: bool,
    pub address: String,
    pub amount: f64,
    pub txid: Option<String>,
    pub error: Option<String>,
    pub output: String,
}

#[derive(Deserialize)]
pub struct SettleRequest {
    pub wallet_id: String,
    pub to_address: Option<String>,
}

#[derive(Serialize)]
pub struct SettleResponse {
    pub wallet_id: String,
    pub success: bool,
    pub txid: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct EsploraClient {
    pub esplora_client: std::sync::Arc<esplora_client::AsyncClient>,
}

impl EsploraClient {
    pub fn new(url: &str) -> Result<Self, anyhow::Error> {
        let builder = esplora_client::Builder::new(url);
        let esplora_client = std::sync::Arc::new(builder.build_async()?);
        Ok(Self { esplora_client })
    }

    pub async fn find_outpoints(
        &self,
        address: &bitcoin::Address,
    ) -> Result<Vec<ExplorerUtxo>, anyhow::Error> {
        let script_pubkey = address.script_pubkey();
        let txs = self
            .esplora_client
            .scripthash_txs(&script_pubkey, None)
            .await?;

        let outputs = txs
            .into_iter()
            .flat_map(|tx| {
                let txid = tx.txid;
                tx.vout
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| v.scriptpubkey == script_pubkey)
                    .map(|(i, v)| ExplorerUtxo {
                        outpoint: bitcoin::OutPoint {
                            txid,
                            vout: i as u32,
                        },
                        amount: Amount::from_sat(v.value),
                        confirmation_blocktime: tx.status.block_time,
                        is_spent: false,
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        let mut utxos = Vec::new();
        for output in outputs.iter() {
            let outpoint = output.outpoint;
            let status = self
                .esplora_client
                .get_output_status(&outpoint.txid, outpoint.vout as u64)
                .await?;

            match status {
                Some(esplora_client::OutputStatus { spent: false, .. }) | None => {
                    utxos.push(*output);
                }
                Some(esplora_client::OutputStatus { spent: true, .. }) => {
                    utxos.push(ExplorerUtxo {
                        is_spent: true,
                        ..*output
                    });
                }
            }
        }

        Ok(utxos)
    }
} 