use actix_web::{App, HttpResponse, HttpServer, Responder, get, post, web};
use anyhow::Result;
use ark_core::ArkAddress;
use ark_core::BoardingOutput;
use ark_core::Vtxo;
use bitcoin::key::Secp256k1;
use bitcoin::secp256k1::PublicKey;
use bitcoin::secp256k1::SecretKey;
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

async fn initialize_server(config: Config) -> Result<ark_core::server::Info> {
    // Connect to Ark server
    let mut grpc_client = ark_grpc::Client::new(config.ark_server_url.clone());
    grpc_client.connect().await?;

    // Get server info
    let server_info = grpc_client.get_info().await?;

    Ok(server_info)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
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

    // Initialize server connection
    let server_info = match initialize_server(config.clone()).await {
        Ok(info) => Some(Mutex::new(info)),
        Err(e) => {
            eprintln!("Failed to connect to Ark server: {}", e);
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
        config,
        server_info,
    });

    println!("Starting Ark API server on 127.0.0.1:8080");

    // Start HTTP server
    HttpServer::new(move || {
        App::new()
            .app_data(app_data.clone())
            .service(create_wallet)
            .service(get_address)
    })
    .bind("127.0.0.1:8080")?
    .run()
    .await
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
