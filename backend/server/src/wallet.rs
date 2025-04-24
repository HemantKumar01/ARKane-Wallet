use actix_web::{get, post, web, HttpResponse, Responder};
use bitcoin::key::Keypair;
use bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
use rand::thread_rng;
use std::collections::HashMap;
use std::str::FromStr;
use uuid::Uuid;

use crate::types::*;
use ark_core::{BoardingOutput, Vtxo};
use ark_core::vtxo::list_virtual_tx_outpoints;
use ark_core::boarding_output::list_boarding_outpoints;
pub use ark_core::ExplorerUtxo;

#[post("/create_wallet")]
pub async fn create_wallet(data: web::Data<AppState>) -> impl Responder {
    let mut rng = thread_rng();
    let secp = Secp256k1::new();
    let keypair = Keypair::new(&secp, &mut rng);
    let secret_key = keypair.secret_key();

    let wallet_id = Uuid::new_v4().to_string();

    let wallet_info = WalletInfo {
        id: wallet_id.clone(),
        seed: secret_key.display_secret().to_string(),
    };

    let mut wallets = data.wallets.lock().unwrap();
    wallets.insert(wallet_id.clone(), wallet_info);

    HttpResponse::Ok().json(WalletResponse { wallet_id })
}

#[get("/get_address/{wallet_id}")]
pub async fn get_address(wallet_id: web::Path<String>, data: web::Data<AppState>) -> impl Responder {
    let wallets = data.wallets.lock().unwrap();
    let wallet_info = match wallets.get(&wallet_id.into_inner()) {
        Some(info) => info.clone(),
        None => return HttpResponse::NotFound().body("Wallet not found"),
    };

    let server_info = match data.server_info.as_ref() {
        Some(info) => info.lock().unwrap().clone(),
        None => return HttpResponse::InternalServerError().body("Server not connected"),
    };

    let sk = match SecretKey::from_str(&wallet_info.seed) {
        Ok(sk) => sk,
        Err(_) => return HttpResponse::InternalServerError().body("Invalid wallet seed"),
    };

    let secp = Secp256k1::new();
    let pk = PublicKey::from_secret_key(&secp, &sk);

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

    let onchain_address = boarding_output.address().to_string();
    let offchain_address = vtxo.to_ark_address().to_string();

    HttpResponse::Ok().json(AddressResponse {
        wallet_id: wallet_info.id,
        onchain_address,
        offchain_address,
    })
}

#[get("/get_balance/{wallet_id}")]
pub async fn get_balance(wallet_id: web::Path<String>, data: web::Data<AppState>) -> impl Responder {
    let wallets = data.wallets.lock().unwrap();
    let wallet_info = match wallets.get(&wallet_id.into_inner()) {
        Some(info) => info.clone(),
        None => return HttpResponse::NotFound().body("Wallet not found"),
    };

    let server_info = match data.server_info.as_ref() {
        Some(info) => info.lock().unwrap().clone(),
        None => return HttpResponse::InternalServerError().body("Server not connected"),
    };

    let esplora_client = match data.esplora_client.as_ref() {
        Some(client) => client.lock().unwrap().clone(),
        None => return HttpResponse::InternalServerError().body("Esplora client not available"),
    };

    let sk = match SecretKey::from_str(&wallet_info.seed) {
        Ok(sk) => sk,
        Err(_) => return HttpResponse::InternalServerError().body("Invalid wallet seed"),
    };

    let secp = Secp256k1::new();
    let pk = PublicKey::from_secret_key(&secp, &sk);

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

    let mut grpc_client = ark_grpc::Client::new(data.config.ark_server_url.clone());
    if let Err(_) = grpc_client.connect().await {
        return HttpResponse::InternalServerError().body("Failed to connect to Ark server");
    }

    let vtxos = match grpc_client.list_vtxos(&vtxo.to_ark_address()).await {
        Ok(vtxos) => vtxos,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to list VTXOs: {}", e));
        }
    };

    let mut spendable_vtxos = HashMap::new();
    spendable_vtxos.insert(vtxo.clone(), vtxos.spendable);

    let boarding_address = boarding_output.address();
    let boarding_outpoints = match esplora_client.find_outpoints(&boarding_address).await {
        Ok(outpoints) => outpoints,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to fetch boarding outpoints: {}", e));
        }
    };

    let mut outpoint_cache = HashMap::new();
    outpoint_cache.insert(boarding_address.to_string(), boarding_outpoints);

    let find_outpoints =
        move |address: &bitcoin::Address| -> Result<Vec<ExplorerUtxo>, ark_core::Error> {
            let address_str = address.to_string();
            match outpoint_cache.get(&address_str) {
                Some(outpoints) => Ok(outpoints.clone()),
                None => Ok(Vec::new()),
            }
        };

    let virtual_tx_outpoints =
        match list_virtual_tx_outpoints(find_outpoints.clone(), spendable_vtxos) {
            Ok(outpoints) => outpoints,
            Err(e) => {
                return HttpResponse::InternalServerError()
                    .body(format!("Failed to get virtual tx outpoints: {}", e));
            }
        };

    let boarding_outpoints = match list_boarding_outpoints(find_outpoints, &[boarding_output]) {
        Ok(outpoints) => outpoints,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to get boarding outpoints: {}", e));
        }
    };

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