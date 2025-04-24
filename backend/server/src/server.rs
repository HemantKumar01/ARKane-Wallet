use actix_web::{App, HttpServer, web};
use anyhow::Result;
use std::fs;
use std::path::Path;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::types::{AppState, Config, EsploraClient};
use crate::wallet::{create_wallet, get_address, get_balance};
use crate::transactions::{send_to_ark_address, faucet, settle_funds};

pub async fn initialize_server(config: Config) -> Result<ark_core::server::Info> {
    let mut grpc_client = ark_grpc::Client::new(config.ark_server_url.clone());
    grpc_client.connect().await?;
    let server_info = grpc_client.get_info().await?;
    Ok(server_info)
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

pub async fn start_server(config: Config) -> std::io::Result<()> {
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
} 