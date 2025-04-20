use actix_web::{App, HttpResponse, HttpServer, Responder, get, post, web};
use anyhow::Result;
use ark_core::ArkAddress;
use ark_core::BoardingOutput;
use ark_core::ExplorerUtxo;
use ark_core::Vtxo;
use ark_core::boarding_output::BoardingOutpoints;
use ark_core::boarding_output::list_boarding_outpoints;
use ark_core::coin_select::select_vtxos;
use ark_core::redeem;
use ark_core::redeem::{build_redeem_transaction, sign_redeem_transaction};
use ark_core::round;
use ark_core::round::create_and_sign_forfeit_txs;
use ark_core::round::generate_nonce_tree;
use ark_core::round::sign_round_psbt;
use ark_core::round::sign_vtxo_tree;
use ark_core::server::{RoundInput, RoundOutput, RoundStreamEvent};
use ark_core::vtxo::VirtualTxOutpoints;
use ark_core::vtxo::list_virtual_tx_outpoints;
use bitcoin::Amount;
use bitcoin::Txid;
use bitcoin::XOnlyPublicKey;
use bitcoin::key::{Keypair, Secp256k1};
use bitcoin::secp256k1::{Message, PublicKey, SecretKey, schnorr};
use futures::StreamExt;
use rand::thread_rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::str::FromStr;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
struct ArkAddressCli(ArkAddress);

impl FromStr for ArkAddressCli {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let address = ArkAddress::decode(s)?;

        Ok(Self(address))
    }
}

#[derive(Deserialize, Clone)]
struct Config {
    ark_server_url: String,
    esplora_url: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct WalletInfo {
    id: String,
    seed: String,
}

struct AppState {
    wallets: Mutex<HashMap<String, WalletInfo>>,
    config: Config,
    server_info: Option<Mutex<ark_core::server::Info>>,
    esplora_client: Option<Mutex<EsploraClient>>,
}

#[derive(Serialize)]
struct AddressResponse {
    wallet_id: String,
    onchain_address: String,
    offchain_address: String,
}

#[derive(Serialize)]
struct WalletResponse {
    wallet_id: String,
}

#[derive(Serialize)]
struct BalanceResponse {
    wallet_id: String,
    offchain_balance: OffchainBalance,
    boarding_balance: BoardingBalance,
}

#[derive(Serialize)]
struct OffchainBalance {
    spendable: u64,
    expired: u64,
}

#[derive(Serialize)]
struct BoardingBalance {
    spendable: u64,
    expired: u64,
    pending: u64,
}

#[derive(Deserialize)]
struct SendToArkAddressRequest {
    wallet_id: String,
    address: String,
    amount: u64,
}

#[derive(Serialize)]
struct SendToArkAddressResponse {
    wallet_id: String,
    to_address: String,
    amount: u64,
    txid: String,
}

#[derive(Clone)]
pub struct EsploraClient {
    esplora_client: std::sync::Arc<esplora_client::AsyncClient>,
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

#[post("/create_wallet")]
async fn create_wallet(data: web::Data<AppState>) -> impl Responder {
    // Generate a random seed for the wallet
    let mut rng = thread_rng();
    let secp = Secp256k1::new();
    let keypair = bitcoin::key::Keypair::new(&secp, &mut rng);
    let secret_key = keypair.secret_key();

    // Create wallet ID
    let wallet_id = Uuid::new_v4().to_string();

    // Store wallet info
    let wallet_info = WalletInfo {
        id: wallet_id.clone(),
        seed: secret_key.display_secret().to_string(),
    };

    // Add to application state
    let mut wallets = data.wallets.lock().unwrap();
    wallets.insert(wallet_id.clone(), wallet_info);

    HttpResponse::Ok().json(WalletResponse { wallet_id })
}

#[get("/get_address/{wallet_id}")]
async fn get_address(wallet_id: web::Path<String>, data: web::Data<AppState>) -> impl Responder {
    // Get wallet info
    let wallets = data.wallets.lock().unwrap();
    let wallet_info = match wallets.get(&wallet_id.into_inner()) {
        Some(info) => info.clone(),
        None => return HttpResponse::NotFound().body("Wallet not found"),
    };

    // Connect to Ark server if not already connected
    let server_info = match data.server_info.as_ref() {
        Some(info) => info.lock().unwrap().clone(),
        None => return HttpResponse::InternalServerError().body("Server not connected"),
    };

    // Parse the seed to get the secret key
    let sk = match SecretKey::from_str(&wallet_info.seed) {
        Ok(sk) => sk,
        Err(_) => return HttpResponse::InternalServerError().body("Invalid wallet seed"),
    };

    let secp = Secp256k1::new();
    let pk = PublicKey::from_secret_key(&secp, &sk);

    // Create boarding output for onchain address
    let boarding_output = match BoardingOutput::new(
        &secp,
        server_info.pk.x_only_public_key().0,
        pk.x_only_public_key().0,
        server_info.unilateral_exit_delay,
        server_info.network,
    ) {
        Ok(bo) => bo,
        Err(_) => {
            return HttpResponse::InternalServerError().body("Failed to create boarding output");
        }
    };

    // Create VTXO for offchain address
    let vtxo = match Vtxo::new(
        &secp,
        server_info.pk.x_only_public_key().0,
        pk.x_only_public_key().0,
        vec![],
        server_info.unilateral_exit_delay,
        server_info.network,
    ) {
        Ok(vtxo) => vtxo,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to create VTXO"),
    };

    // Get addresses
    let onchain_address = boarding_output.address().to_string();
    let offchain_address = vtxo.to_ark_address().to_string();

    HttpResponse::Ok().json(AddressResponse {
        wallet_id: wallet_info.id,
        onchain_address,
        offchain_address,
    })
}

#[get("/get_balance/{wallet_id}")]
async fn get_balance(wallet_id: web::Path<String>, data: web::Data<AppState>) -> impl Responder {
    // Get wallet info
    let wallets = data.wallets.lock().unwrap();
    let wallet_info = match wallets.get(&wallet_id.into_inner()) {
        Some(info) => info.clone(),
        None => return HttpResponse::NotFound().body("Wallet not found"),
    };

    // Get Ark server info and Esplora client
    let server_info = match data.server_info.as_ref() {
        Some(info) => info.lock().unwrap().clone(),
        None => return HttpResponse::InternalServerError().body("Server not connected"),
    };

    let esplora_client = match data.esplora_client.as_ref() {
        Some(client) => client.lock().unwrap().clone(),
        None => return HttpResponse::InternalServerError().body("Esplora client not available"),
    };

    // Parse the seed to get the secret key and public key
    let sk = match SecretKey::from_str(&wallet_info.seed) {
        Ok(sk) => sk,
        Err(_) => return HttpResponse::InternalServerError().body("Invalid wallet seed"),
    };

    let secp = Secp256k1::new();
    let pk = PublicKey::from_secret_key(&secp, &sk);

    // Create boarding output and VTXO
    let boarding_output = match BoardingOutput::new(
        &secp,
        server_info.pk.x_only_public_key().0,
        pk.x_only_public_key().0,
        server_info.unilateral_exit_delay,
        server_info.network,
    ) {
        Ok(bo) => bo,
        Err(_) => {
            return HttpResponse::InternalServerError().body("Failed to create boarding output");
        }
    };

    let vtxo = match Vtxo::new(
        &secp,
        server_info.pk.x_only_public_key().0,
        pk.x_only_public_key().0,
        vec![],
        server_info.unilateral_exit_delay,
        server_info.network,
    ) {
        Ok(vtxo) => vtxo,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to create VTXO"),
    };

    // Create a client for connecting to the Ark server
    let mut grpc_client = ark_grpc::Client::new(data.config.ark_server_url.clone());

    // Connect to the Ark server
    if let Err(_) = grpc_client.connect().await {
        return HttpResponse::InternalServerError().body("Failed to connect to Ark server");
    }

    // Get VTXOs directly in the async context
    let vtxos = match grpc_client.list_vtxos(&vtxo.to_ark_address()).await {
        Ok(vtxos) => vtxos,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to list VTXOs: {}", e));
        }
    };

    // Create a HashMap with the spendable VTXOs
    let mut spendable_vtxos = HashMap::new();
    spendable_vtxos.insert(vtxo.clone(), vtxos.spendable);

    // Step 1: Prefetch all outpoints we'll need
    // First, get the addresses for which we need outpoints
    let boarding_address = boarding_output.address();

    // Fetch outpoints for boarding output
    let boarding_outpoints = match esplora_client.find_outpoints(&boarding_address).await {
        Ok(outpoints) => outpoints,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to fetch boarding outpoints: {}", e));
        }
    };

    // Create a cache of outpoints
    let mut outpoint_cache = HashMap::new();
    outpoint_cache.insert(boarding_address.to_string(), boarding_outpoints);

    // Create a closure that uses the prefetched outpoints
    let find_outpoints =
        move |address: &bitcoin::Address| -> Result<Vec<ExplorerUtxo>, ark_core::Error> {
            let address_str = address.to_string();
            match outpoint_cache.get(&address_str) {
                Some(outpoints) => Ok(outpoints.clone()),
                None => Ok(Vec::new()), // Fallback for any addresses we didn't prefetch
            }
        };

    // Get virtual tx outpoints
    let virtual_tx_outpoints =
        match list_virtual_tx_outpoints(find_outpoints.clone(), spendable_vtxos) {
            Ok(outpoints) => outpoints,
            Err(e) => {
                return HttpResponse::InternalServerError()
                    .body(format!("Failed to get virtual tx outpoints: {}", e));
            }
        };

    // Get boarding outpoints
    let boarding_outpoints = match list_boarding_outpoints(find_outpoints, &[boarding_output]) {
        Ok(outpoints) => outpoints,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to get boarding outpoints: {}", e));
        }
    };

    // Create balance response
    let response = BalanceResponse {
        wallet_id: wallet_info.id,
        offchain_balance: OffchainBalance {
            spendable: virtual_tx_outpoints.spendable_balance().to_sat(),
            expired: virtual_tx_outpoints.expired_balance().to_sat(),
        },
        boarding_balance: BoardingBalance {
            spendable: boarding_outpoints.spendable_balance().to_sat(),
            expired: boarding_outpoints.expired_balance().to_sat(),
            pending: boarding_outpoints.pending_balance().to_sat(),
        },
    };

    HttpResponse::Ok().json(response)
}

#[post("/send_to_ark_address")]
async fn send_to_ark_address(
    data: web::Data<AppState>,
    req: web::Json<SendToArkAddressRequest>,
) -> impl Responder {
    // Get wallet info
    let wallets = data.wallets.lock().unwrap();
    let wallet_info = match wallets.get(&req.wallet_id) {
        Some(info) => info.clone(),
        None => return HttpResponse::NotFound().body("Wallet not found"),
    };

    // Connect to Ark server if not already connected
    let server_info = match data.server_info.as_ref() {
        Some(info) => info.lock().unwrap().clone(),
        None => return HttpResponse::InternalServerError().body("Server not connected"),
    };

    // Parse the seed to get the secret key
    let sk = match SecretKey::from_str(&wallet_info.seed) {
        Ok(sk) => sk,
        Err(_) => return HttpResponse::InternalServerError().body("Invalid wallet seed"),
    };

    // Parse the destination address
    let destination_address = match ArkAddress::decode(&req.address) {
        Ok(address) => address,
        Err(_) => return HttpResponse::BadRequest().body("Invalid Ark address"),
    };

    // Convert amount to bitcoin::Amount
    let amount = Amount::from_sat(req.amount);

    // Create VTXO
    let secp = Secp256k1::new();
    let pk = PublicKey::from_secret_key(&secp, &sk);

    let vtxo = match Vtxo::new(
        &secp,
        server_info.pk.x_only_public_key().0,
        pk.x_only_public_key().0,
        vec![],
        server_info.unilateral_exit_delay,
        server_info.network,
    ) {
        Ok(vtxo) => vtxo,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to create VTXO"),
    };

    // Create a client for connecting to the Ark server
    let mut grpc_client = ark_grpc::Client::new(data.config.ark_server_url.clone());

    // Connect to the Ark server
    if let Err(_) = grpc_client.connect().await {
        return HttpResponse::InternalServerError().body("Failed to connect to Ark server");
    }

    // Get VTXOs directly in the async context
    let vtxos = match grpc_client.list_vtxos(&vtxo.to_ark_address()).await {
        Ok(vtxos) => vtxos,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to list VTXOs"),
    };

    // Create a simplified find_outpoints function that just returns empty results
    // In a real implementation, this would need to be properly asynchronous
    let find_outpoints =
        |_address: &bitcoin::Address| -> Result<Vec<ExplorerUtxo>, ark_core::Error> {
            Ok(Vec::new())
        };

    // Create a HashMap with the spendable VTXOs
    let mut spendable_vtxos = HashMap::new();
    spendable_vtxos.insert(vtxo.clone(), vtxos.spendable);

    // Get virtual tx outpoints
    let virtual_tx_outpoints = match list_virtual_tx_outpoints(find_outpoints, spendable_vtxos) {
        Ok(outpoints) => outpoints,
        Err(_) => {
            return HttpResponse::InternalServerError().body("Failed to get virtual tx outpoints");
        }
    };

    // Extract VTXO outpoints for coin selection
    let vtxo_outpoints = virtual_tx_outpoints
        .spendable
        .iter()
        .map(|(outpoint, _)| ark_core::coin_select::VtxoOutPoint {
            outpoint: outpoint.outpoint,
            expire_at: outpoint.expire_at,
            amount: outpoint.amount,
        })
        .collect::<Vec<_>>();

    // Select outpoints for the transaction
    let selected_outpoints = match select_vtxos(vtxo_outpoints, amount, server_info.dust, true) {
        Ok(outpoints) => outpoints,
        Err(_) => return HttpResponse::BadRequest().body("Insufficient funds or invalid amount"),
    };

    // Filter and map the selected outpoints to VtxoInput objects
    let vtxo_inputs = virtual_tx_outpoints
        .spendable
        .into_iter()
        .filter(|(outpoint, _)| {
            selected_outpoints
                .iter()
                .any(|o| o.outpoint == outpoint.outpoint)
        })
        .map(|(outpoint, vtxo)| redeem::VtxoInput::new(vtxo, outpoint.amount, outpoint.outpoint))
        .collect::<Vec<_>>();

    // Generate a change address using the current VTXO
    let change_address = vtxo.to_ark_address();

    // Create the keypair from the secret key
    let secp = Secp256k1::new();
    let kp = Keypair::from_secret_key(&secp, &sk);

    // Build the redeem transaction PSBT
    let mut redeem_psbt = match build_redeem_transaction(
        &[(&destination_address, amount)],
        Some(&change_address),
        &vtxo_inputs,
    ) {
        Ok(psbt) => psbt,
        Err(_) => {
            return HttpResponse::InternalServerError().body("Failed to build redeem transaction");
        }
    };

    // Define a signing function for the transaction
    let sign_fn = |msg: Message| -> Result<(schnorr::Signature, XOnlyPublicKey), ark_core::Error> {
        let sig = Secp256k1::new().sign_schnorr_no_aux_rand(&msg, &kp);
        let pk = kp.x_only_public_key().0;
        Ok((sig, pk))
    };

    // Sign the transaction inputs
    for (i, _) in vtxo_inputs.iter().enumerate() {
        if let Err(_) = sign_redeem_transaction(sign_fn, &mut redeem_psbt, &vtxo_inputs, i) {
            return HttpResponse::InternalServerError().body("Failed to sign redeem transaction");
        }
    }

    // Submit the redeem transaction to the Ark server
    let psbt = match grpc_client.submit_redeem_transaction(redeem_psbt).await {
        Ok(psbt) => psbt,
        Err(_) => {
            return HttpResponse::InternalServerError().body("Failed to submit redeem transaction");
        }
    };

    // Extract the transaction ID
    let txid = match psbt.extract_tx() {
        Ok(tx) => tx.compute_txid().to_string(),
        Err(_) => return HttpResponse::InternalServerError().body("Failed to extract transaction"),
    };

    // Return the transaction result
    HttpResponse::Ok().json(SendToArkAddressResponse {
        wallet_id: wallet_info.id,
        to_address: req.address.clone(),
        amount: req.amount,
        txid,
    })
}
#[derive(Deserialize)]
struct FaucetRequest {
    onchain_address: String,
    amount: f64, // Using f64 as nigiri faucet might accept decimal amounts
}

// Define response struct for the faucet endpoint
#[derive(Serialize)]
struct FaucetResponse {
    success: bool,
    address: String,
    amount: f64,
    txid: Option<String>,
    error: Option<String>,
    output: String,
}

// Add this new route to your existing imports and function declarations
#[post("/faucet")]
async fn faucet(req: web::Json<FaucetRequest>) -> impl Responder {
    // Validate input parameters
    if req.onchain_address.is_empty() {
        return HttpResponse::BadRequest().json(FaucetResponse {
            success: false,
            address: req.onchain_address.clone(),
            amount: req.amount,
            txid: None,
            error: Some("Empty onchain address provided".to_string()),
            output: String::new(),
        });
    }

    if req.amount <= 0.0 {
        return HttpResponse::BadRequest().json(FaucetResponse {
            success: false,
            address: req.onchain_address.clone(),
            amount: req.amount,
            txid: None,
            error: Some("Amount must be greater than zero".to_string()),
            output: String::new(),
        });
    }

    // Execute nigiri faucet command
    let output = Command::new("nigiri")
        .arg("faucet")
        .arg(&req.onchain_address)
        .arg(req.amount.to_string())
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            // Check if the command was successful
            if output.status.success() {
                // Try to extract txid from the output
                let txid = extract_txid_from_output(&stdout);

                HttpResponse::Ok().json(FaucetResponse {
                    success: true,
                    address: req.onchain_address.clone(),
                    amount: req.amount,
                    txid,
                    error: None,
                    output: stdout,
                })
            } else {
                HttpResponse::InternalServerError().json(FaucetResponse {
                    success: false,
                    address: req.onchain_address.clone(),
                    amount: req.amount,
                    txid: None,
                    error: Some(format!("Command failed: {}", stderr)),
                    output: stdout,
                })
            }
        }
        Err(e) => HttpResponse::InternalServerError().json(FaucetResponse {
            success: false,
            address: req.onchain_address.clone(),
            amount: req.amount,
            txid: None,
            error: Some(format!("Failed to execute command: {}", e)),
            output: String::new(),
        }),
    }
}

// Helper function to try extracting a txid from command output
fn extract_txid_from_output(output: &str) -> Option<String> {
    // This regex will try to match common Bitcoin transaction ID patterns
    // Adjust as needed based on the actual format of nigiri's output
    let re = regex::Regex::new(r"[0-9a-f]{64}").ok()?;
    re.find(output).map(|m| m.as_str().to_string())
}
async fn initialize_server(config: Config) -> Result<ark_core::server::Info> {
    // Connect to Ark server
    let mut grpc_client = ark_grpc::Client::new(config.ark_server_url.clone());
    grpc_client.connect().await?;

    // Get server info
    let server_info = grpc_client.get_info().await?;

    Ok(server_info)
}
#[derive(Deserialize)]
struct SettleRequest {
    wallet_id: String,
    to_address: Option<String>, // Optional, will use wallet's own address if not provided
}

#[derive(Serialize)]
struct SettleResponse {
    wallet_id: String,
    success: bool,
    txid: Option<String>,
    error: Option<String>,
}

#[post("/settle")]
async fn settle_funds(data: web::Data<AppState>, req: web::Json<SettleRequest>) -> impl Responder {
    // Get wallet info
    let wallets = data.wallets.lock().unwrap();
    let wallet_info = match wallets.get(&req.wallet_id) {
        Some(info) => info.clone(),
        None => return HttpResponse::NotFound().body("Wallet not found"),
    };

    // Connect to Ark server if not already connected
    let server_info = match data.server_info.as_ref() {
        Some(info) => info.lock().unwrap().clone(),
        None => return HttpResponse::InternalServerError().body("Server not connected"),
    };

    // Get Esplora client
    let esplora_client = match data.esplora_client.as_ref() {
        Some(client) => client.lock().unwrap().clone(),
        None => return HttpResponse::InternalServerError().body("Esplora client not available"),
    };

    // Parse the seed to get the secret key
    let sk = match SecretKey::from_str(&wallet_info.seed) {
        Ok(sk) => sk,
        Err(_) => return HttpResponse::InternalServerError().body("Invalid wallet seed"),
    };

    // Create the secp context and get the public key
    let secp = Secp256k1::new();
    let pk = PublicKey::from_secret_key(&secp, &sk);

    // Create boarding output and VTXO
    let boarding_output = match BoardingOutput::new(
        &secp,
        server_info.pk.x_only_public_key().0,
        pk.x_only_public_key().0,
        server_info.unilateral_exit_delay,
        server_info.network,
    ) {
        Ok(bo) => bo,
        Err(_) => {
            return HttpResponse::InternalServerError().body("Failed to create boarding output");
        }
    };

    let vtxo = match Vtxo::new(
        &secp,
        server_info.pk.x_only_public_key().0,
        pk.x_only_public_key().0,
        vec![],
        server_info.unilateral_exit_delay,
        server_info.network,
    ) {
        Ok(vtxo) => vtxo,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to create VTXO"),
    };

    // Create a client for connecting to the Ark server
    let mut grpc_client = ark_grpc::Client::new(data.config.ark_server_url.clone());

    // Connect to the Ark server
    if let Err(_) = grpc_client.connect().await {
        return HttpResponse::InternalServerError().body("Failed to connect to Ark server");
    }

    // Step 1: Prefetch all outpoints we'll need
    // First, get the addresses for which we need outpoints
    let boarding_address = boarding_output.address();

    // Fetch outpoints for boarding output
    let boarding_outpoints = match esplora_client.find_outpoints(&boarding_address).await {
        Ok(outpoints) => outpoints,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to fetch boarding outpoints: {}", e));
        }
    };

    // Create a cache of outpoints
    let mut outpoint_cache = HashMap::new();
    outpoint_cache.insert(boarding_address.to_string(), boarding_outpoints);

    // Create a closure that uses the prefetched outpoints
    let find_outpoints =
        move |address: &bitcoin::Address| -> Result<Vec<ExplorerUtxo>, ark_core::Error> {
            let address_str = address.to_string();
            match outpoint_cache.get(&address_str) {
                Some(outpoints) => Ok(outpoints.clone()),
                None => Ok(Vec::new()), // Fallback for any addresses we didn't prefetch
            }
        };

    // Get VTXOs directly in the async context
    let vtxos = match grpc_client.list_vtxos(&vtxo.to_ark_address()).await {
        Ok(vtxos) => vtxos,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to list VTXOs: {}", e));
        }
    };

    // Create a HashMap with the spendable VTXOs
    let mut spendable_vtxos = HashMap::new();
    spendable_vtxos.insert(vtxo.clone(), vtxos.spendable);

    // Get virtual tx outpoints
    let virtual_tx_outpoints =
        match list_virtual_tx_outpoints(find_outpoints.clone(), spendable_vtxos) {
            Ok(outpoints) => outpoints,
            Err(e) => {
                return HttpResponse::InternalServerError()
                    .body(format!("Failed to get virtual tx outpoints: {}", e));
            }
        };

    // Get boarding outpoints
    let boarding_outpoints = match list_boarding_outpoints(find_outpoints, &[boarding_output]) {
        Ok(outpoints) => outpoints,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to get boarding outpoints: {}", e));
        }
    };

    // Debug: Print available balances
    let vtxo_spendable = virtual_tx_outpoints.spendable_balance().to_sat();
    let vtxo_expired = virtual_tx_outpoints.expired_balance().to_sat();
    let boarding_spendable = boarding_outpoints.spendable_balance().to_sat();
    let boarding_expired = boarding_outpoints.expired_balance().to_sat();
    let boarding_pending = boarding_outpoints.pending_balance().to_sat();
    let total_spendable = vtxo_spendable + boarding_spendable;

    println!("=== SETTLEMENT BALANCE INFORMATION ===");
    println!("Wallet ID: {}", wallet_info.id);
    println!("VTXO spendable balance: {} sats", vtxo_spendable);
    println!("VTXO expired balance: {} sats", vtxo_expired);
    println!("Boarding spendable balance: {} sats", boarding_spendable);
    println!("Boarding expired balance: {} sats", boarding_expired);
    println!("Boarding pending balance: {} sats", boarding_pending);
    println!("Total spendable balance: {} sats", total_spendable);
    println!("=======================================");

    // Count the number of spendable VTXOs and boarding outputs
    println!(
        "Number of spendable VTXOs: {}",
        virtual_tx_outpoints.spendable.len()
    );
    println!(
        "Number of spendable boarding outputs: {}",
        boarding_outpoints.spendable.len()
    );

    // Determine the destination address
    let to_address = match &req.to_address {
        Some(addr) => match ArkAddress::decode(addr) {
            Ok(address) => address,
            Err(_) => return HttpResponse::BadRequest().body("Invalid destination Ark address"),
        },
        None => vtxo.to_ark_address(), // Use wallet's own address as default
    };

    println!("Settlement destination address: {}", to_address);

    // Call the settle function
    let settle_result = settle_internal(
        &grpc_client,
        &server_info,
        sk,
        virtual_tx_outpoints,
        boarding_outpoints,
        to_address,
    )
    .await;

    match settle_result {
        Ok(Some(txid)) => {
            println!("Settlement successful! TXID: {}", txid);
            HttpResponse::Ok().json(SettleResponse {
                wallet_id: wallet_info.id,
                success: true,
                txid: Some(txid.to_string()),
                error: None,
            })
        }
        Ok(None) => {
            println!("Settlement failed: No spendable outputs available");
            HttpResponse::Ok().json(SettleResponse {
                wallet_id: wallet_info.id,
                success: false,
                txid: None,
                error: Some(
                    "No boarding outputs or VTXOs can be settled at the moment".to_string(),
                ),
            })
        }
        Err(e) => {
            println!("Settlement error: {}", e);
            HttpResponse::InternalServerError().json(SettleResponse {
                wallet_id: wallet_info.id,
                success: false,
                txid: None,
                error: Some(format!("Failed to settle: {}", e)),
            })
        }
    }
}

// Implement a version of the settle function from sample.rs
async fn settle_internal(
    grpc_client: &ark_grpc::Client,
    server_info: &ark_core::server::Info,
    sk: SecretKey,
    vtxos: VirtualTxOutpoints,
    boarding_outputs: BoardingOutpoints,
    to_address: ArkAddress,
) -> Result<Option<Txid>, anyhow::Error> {
    let secp = Secp256k1::new();
    let mut rng = thread_rng();

    if vtxos.spendable.is_empty() && boarding_outputs.spendable.is_empty() {
        return Ok(None);
    }

    let cosigner_kp = Keypair::new(&secp, &mut rng);

    let round_inputs = {
        let boarding_inputs = boarding_outputs
            .spendable
            .clone()
            .into_iter()
            .map(|o| RoundInput::new(o.0, o.2.tapscripts()));

        let vtxo_inputs = vtxos
            .spendable
            .clone()
            .into_iter()
            .map(|v| RoundInput::new(v.0.outpoint, v.1.tapscripts()));

        boarding_inputs.chain(vtxo_inputs).collect::<Vec<_>>()
    };

    let payment_id = grpc_client
        .register_inputs_for_next_round(&round_inputs)
        .await?;

    let spendable_amount = boarding_outputs.spendable_balance() + vtxos.spendable_balance();

    let round_outputs = vec![RoundOutput::new_virtual(to_address, spendable_amount)];
    grpc_client
        .register_outputs_for_next_round(
            payment_id.clone(),
            &round_outputs,
            &[cosigner_kp.public_key()],
            false,
        )
        .await?;

    grpc_client.ping(payment_id).await?;

    let mut event_stream = grpc_client.get_event_stream().await?;

    let round_signing_event = match event_stream.next().await {
        Some(Ok(RoundStreamEvent::RoundSigning(e))) => e,
        other => {
            return Err(anyhow::anyhow!(
                "Did not get round signing event: {:?}",
                other
            ));
        }
    };

    let round_id = round_signing_event.id;

    let unsigned_vtxo_tree = round_signing_event
        .unsigned_vtxo_tree
        .expect("to have an unsigned VTXO tree");

    let nonce_tree = generate_nonce_tree(&mut rng, &unsigned_vtxo_tree, cosigner_kp.public_key())?;

    grpc_client
        .submit_tree_nonces(
            &round_id,
            cosigner_kp.public_key(),
            nonce_tree.to_pub_nonce_tree().into_inner(),
        )
        .await?;

    let round_signing_nonces_generated_event = match event_stream.next().await {
        Some(Ok(RoundStreamEvent::RoundSigningNoncesGenerated(e))) => e,
        other => {
            return Err(anyhow::anyhow!(
                "Did not get round signing nonces generated event: {:?}",
                other
            ));
        }
    };

    let round_id = round_signing_nonces_generated_event.id;
    let agg_pub_nonce_tree = round_signing_nonces_generated_event.tree_nonces;

    let partial_sig_tree = sign_vtxo_tree(
        server_info.vtxo_tree_expiry,
        server_info.pk.x_only_public_key().0,
        &cosigner_kp,
        &unsigned_vtxo_tree,
        &round_signing_event.unsigned_round_tx,
        nonce_tree,
        &agg_pub_nonce_tree.into(),
    )?;

    grpc_client
        .submit_tree_signatures(
            &round_id,
            cosigner_kp.public_key(),
            partial_sig_tree.into_inner(),
        )
        .await?;

    let round_finalization_event = match event_stream.next().await {
        Some(Ok(RoundStreamEvent::RoundFinalization(e))) => e,
        other => {
            return Err(anyhow::anyhow!(
                "Did not get round finalization event: {:?}",
                other
            ));
        }
    };

    let round_id = round_finalization_event.id;

    let vtxo_inputs = vtxos
        .spendable
        .into_iter()
        .map(|(outpoint, vtxo)| round::VtxoInput::new(vtxo, outpoint.amount, outpoint.outpoint))
        .collect::<Vec<_>>();

    let keypair = Keypair::from_secret_key(&secp, &sk);
    let signed_forfeit_psbts = create_and_sign_forfeit_txs(
        &keypair,
        vtxo_inputs.as_slice(),
        round_finalization_event.connector_tree,
        &round_finalization_event.connectors_index,
        round_finalization_event.min_relay_fee_rate,
        &server_info.forfeit_address,
        server_info.dust,
    )?;

    let onchain_inputs = boarding_outputs
        .spendable
        .into_iter()
        .map(|(outpoint, _, boarding_output)| round::OnChainInput::new(boarding_output, outpoint))
        .collect::<Vec<_>>();

    let round_psbt = if round_inputs.is_empty() {
        None
    } else {
        let mut round_psbt = round_finalization_event.round_tx;

        let sign_for_pk_fn =
            |_: &XOnlyPublicKey, msg: &Message| -> Result<schnorr::Signature, ark_core::Error> {
                Ok(secp.sign_schnorr_no_aux_rand(msg, &keypair))
            };

        sign_round_psbt(sign_for_pk_fn, &mut round_psbt, &onchain_inputs)?;

        Some(round_psbt)
    };

    grpc_client
        .submit_signed_forfeit_txs(signed_forfeit_psbts, round_psbt)
        .await?;

    let round_finalized_event = match event_stream.next().await {
        Some(Ok(RoundStreamEvent::RoundFinalized(e))) => e,
        other => {
            return Err(anyhow::anyhow!(
                "Did not get round finalized event: {:?}",
                other
            ));
        }
    };

    let round_id = round_finalized_event.id;
    Ok(Some(round_finalized_event.round_txid))
}
fn main() -> std::io::Result<()> {
    init_tracing();

    // Load configuration
    let config = match fs::read_to_string("ark.config.toml") {
        Ok(content) => match toml::from_str::<Config>(&content) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Failed to parse config: {}", e);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Config parse error",
                ));
            }
        },
        Err(e) => {
            eprintln!("Failed to read config file: {}", e);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Config read error",
            ));
        }
    };

    // Initialize server connection and other startup tasks
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            // Initialize server connection
            let server_info = match initialize_server(config.clone()).await {
                Ok(info) => Some(Mutex::new(info)),
                Err(e) => {
                    eprintln!("Failed to connect to Ark server: {}", e);
                    None
                }
            };

            // Initialize Esplora client
            let esplora_client = match EsploraClient::new(&config.esplora_url) {
                Ok(client) => Some(Mutex::new(client)),
                Err(e) => {
                    eprintln!("Failed to create Esplora client: {}", e);
                    None
                }
            };

            // Create directory for wallet storage if it doesn't exist
            if !Path::new("wallets").exists() {
                fs::create_dir("wallets")?;
            }

            // Set up application state
            let app_data = web::Data::new(AppState {
                wallets: Mutex::new(HashMap::new()),
                config: config.clone(),
                server_info,
                esplora_client,
            });

            println!("Starting Ark API server on 127.0.0.1:8080");

            // Start HTTP server
            HttpServer::new(move || {
                App::new()
                    .app_data(app_data.clone())
                    .service(create_wallet)
                    .service(get_address)
                    .service(get_balance)
                    .service(send_to_ark_address)
                    .service(faucet)
                    .service(settle_funds)
            })
            .bind("127.0.0.1:8080")?
            .run()
            .await
        })
}
pub fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            "debug,\
             tower=info,\
             hyper_util=info,\
             hyper=info,\
             h2=warn,\
             reqwest=info,\
             ark_core=info,\
             rustls=info",
        )
        .init()
}
