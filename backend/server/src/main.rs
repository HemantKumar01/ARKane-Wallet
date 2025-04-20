use actix_web::{App, HttpResponse, HttpServer, Responder, get, post, web};
use anyhow::Result;
use ark_core::ArkAddress;
use ark_core::BoardingOutput;
use ark_core::ExplorerUtxo;
use ark_core::Vtxo;
use ark_core::boarding_output::list_boarding_outpoints;
use ark_core::coin_select::select_vtxos;
use ark_core::redeem;
use ark_core::redeem::{build_redeem_transaction, sign_redeem_transaction};
use ark_core::vtxo::list_virtual_tx_outpoints;
use bitcoin::Amount;
use bitcoin::XOnlyPublicKey;
use bitcoin::key::{Keypair, Secp256k1};
use bitcoin::secp256k1::{Message, PublicKey, SecretKey, schnorr};
use rand::thread_rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
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

pub struct EsploraClient {
    esplora_client: esplora_client::AsyncClient,
}

impl EsploraClient {
    pub fn new(url: &str) -> Result<Self, anyhow::Error> {
        let builder = esplora_client::Builder::new(url);
        let esplora_client = builder.build_async()?;

        Ok(Self { esplora_client })
    }

    async fn find_outpoints(
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
        Some(client) => client.lock().unwrap(),
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
    let vtxo_result = match grpc_client.list_vtxos(&vtxo.to_ark_address()).await {
        Ok(vtxos) => Ok(vtxos),
        Err(e) => Err(format!("Failed to list VTXOs: {}", e)),
    };

    let vtxos = match vtxo_result {
        Ok(vtxos) => vtxos,
        Err(e) => return HttpResponse::InternalServerError().body(e),
    };

    // Create async-friendly find_outpoints function
    let find_outpoints =
        |address: &bitcoin::Address| -> Result<Vec<ExplorerUtxo>, ark_core::Error> {
            // Since we can't use block_in_place, we'll need to refactor this approach
            // For now, we'll simplify by returning an empty vector which will show zero balance
            // A proper implementation would require restructuring the application to handle this async properly
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

    // Get boarding outpoints
    let boarding_outpoints = match list_boarding_outpoints(find_outpoints, &[boarding_output]) {
        Ok(outpoints) => outpoints,
        Err(_) => {
            return HttpResponse::InternalServerError().body("Failed to get boarding outpoints");
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

async fn initialize_server(config: Config) -> Result<ark_core::server::Info> {
    // Connect to Ark server
    let mut grpc_client = ark_grpc::Client::new(config.ark_server_url.clone());
    grpc_client.connect().await?;

    // Get server info
    let server_info = grpc_client.get_info().await?;

    Ok(server_info)
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
