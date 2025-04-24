use actix_web::{post, web, HttpResponse, Responder};
use bitcoin::{Amount, Txid, XOnlyPublicKey};
use bitcoin::key::{Keypair, Secp256k1};
use bitcoin::secp256k1::{Message, PublicKey, SecretKey, schnorr};
use std::collections::HashMap;
use std::str::FromStr;
use std::process::Command;
use futures::StreamExt;
use rand::thread_rng;

use crate::types::*;
use ark_core::{ArkAddress, BoardingOutput, Vtxo};
use ark_core::vtxo::list_virtual_tx_outpoints;
use ark_core::boarding_output::list_boarding_outpoints;
use ark_core::coin_select::select_vtxos;
use ark_core::redeem::{self, build_redeem_transaction, sign_redeem_transaction};
use ark_core::round::{self, create_and_sign_forfeit_txs, generate_nonce_tree, sign_round_psbt, sign_vtxo_tree};
use ark_core::server::{RoundInput, RoundOutput, RoundStreamEvent};
use ark_core::ExplorerUtxo;

#[post("/send_to_ark_address")]
pub async fn send_to_ark_address(
    data: web::Data<AppState>,
    req: web::Json<SendToArkAddressRequest>,
) -> impl Responder {
    let wallets = data.wallets.lock().unwrap();
    let wallet_info = match wallets.get(&req.wallet_id) {
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

    let destination_address = match ArkAddress::decode(&req.address) {
        Ok(address) => address,
        Err(_) => return HttpResponse::BadRequest().body("Invalid Ark address"),
    };

    let amount = Amount::from_sat(req.amount);

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

    let mut grpc_client = ark_grpc::Client::new(data.config.ark_server_url.clone());
    if let Err(_) = grpc_client.connect().await {
        return HttpResponse::InternalServerError().body("Failed to connect to Ark server");
    }

    let vtxos = match grpc_client.list_vtxos(&vtxo.to_ark_address()).await {
        Ok(vtxos) => vtxos,
        Err(_) => return HttpResponse::InternalServerError().body("Failed to list VTXOs"),
    };

    let vtxo_address = vtxo.address();
    let vtxo_explorer_outpoints = match esplora_client.find_outpoints(&vtxo_address).await {
        Ok(outpoints) => outpoints,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to fetch VTXO outpoints: {}", e));
        }
    };

    let mut outpoint_cache = HashMap::new();
    outpoint_cache.insert(vtxo_address.to_string(), vtxo_explorer_outpoints);

    let find_outpoints =
        move |address: &bitcoin::Address| -> Result<Vec<ExplorerUtxo>, ark_core::Error> {
            let address_str = address.to_string();
            match outpoint_cache.get(&address_str) {
                Some(outpoints) => Ok(outpoints.clone()),
                None => Ok(Vec::new()),
            }
        };

    let mut spendable_vtxos = HashMap::new();
    spendable_vtxos.insert(vtxo.clone(), vtxos.spendable);

    let virtual_tx_outpoints = match list_virtual_tx_outpoints(find_outpoints, spendable_vtxos) {
        Ok(outpoints) => outpoints,
        Err(_) => {
            return HttpResponse::InternalServerError().body("Failed to get virtual tx outpoints");
        }
    };

    let vtxo_outpoints = virtual_tx_outpoints
        .spendable
        .iter()
        .map(|(outpoint, _)| ark_core::coin_select::VtxoOutPoint {
            outpoint: outpoint.outpoint,
            expire_at: outpoint.expire_at,
            amount: outpoint.amount,
        })
        .collect::<Vec<_>>();

    let selected_outpoints = match select_vtxos(vtxo_outpoints, amount, server_info.dust, true) {
        Ok(outpoints) => outpoints,
        Err(_) => return HttpResponse::BadRequest().body("Insufficient funds or invalid amount"),
    };

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

    let change_address = vtxo.to_ark_address();

    let secp = Secp256k1::new();
    let kp = Keypair::from_secret_key(&secp, &sk);

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

    let sign_fn = |msg: Message| -> Result<(schnorr::Signature, XOnlyPublicKey), ark_core::Error> {
        let sig = Secp256k1::new().sign_schnorr_no_aux_rand(&msg, &kp);
        let pk = kp.x_only_public_key().0;
        Ok((sig, pk))
    };

    for (i, _) in vtxo_inputs.iter().enumerate() {
        if let Err(_) = sign_redeem_transaction(sign_fn, &mut redeem_psbt, &vtxo_inputs, i) {
            return HttpResponse::InternalServerError().body("Failed to sign redeem transaction");
        }
    }

    let psbt = match grpc_client.submit_redeem_transaction(redeem_psbt).await {
        Ok(psbt) => psbt,
        Err(_) => {
            return HttpResponse::InternalServerError().body("Failed to submit redeem transaction");
        }
    };

    let txid = match psbt.extract_tx() {
        Ok(tx) => tx.compute_txid().to_string(),
        Err(_) => return HttpResponse::InternalServerError().body("Failed to extract transaction"),
    };

    HttpResponse::Ok().json(SendToArkAddressResponse {
        wallet_id: wallet_info.id,
        to_address: req.address.clone(),
        amount: req.amount,
        txid,
    })
}

#[post("/faucet")]
pub async fn faucet(req: web::Json<FaucetRequest>) -> impl Responder {
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

    let output = Command::new("nigiri")
        .arg("faucet")
        .arg(&req.onchain_address)
        .arg(req.amount.to_string())
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            if output.status.success() {
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

fn extract_txid_from_output(output: &str) -> Option<String> {
    let re = regex::Regex::new(r"[0-9a-f]{64}").ok()?;
    re.find(output).map(|m| m.as_str().to_string())
}

#[post("/settle")]
pub async fn settle_funds(data: web::Data<AppState>, req: web::Json<SettleRequest>) -> impl Responder {
    let wallets = data.wallets.lock().unwrap();
    let wallet_info = match wallets.get(&req.wallet_id) {
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

    let vtxos = match grpc_client.list_vtxos(&vtxo.to_ark_address()).await {
        Ok(vtxos) => vtxos,
        Err(e) => {
            return HttpResponse::InternalServerError()
                .body(format!("Failed to list VTXOs: {}", e));
        }
    };

    let mut spendable_vtxos = HashMap::new();
    spendable_vtxos.insert(vtxo.clone(), vtxos.spendable);

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

    println!(
        "Number of spendable VTXOs: {}",
        virtual_tx_outpoints.spendable.len()
    );
    println!(
        "Number of spendable boarding outputs: {}",
        boarding_outpoints.spendable.len()
    );

    let to_address = match &req.to_address {
        Some(addr) => match ArkAddress::decode(addr) {
            Ok(address) => address,
            Err(_) => return HttpResponse::BadRequest().body("Invalid destination Ark address"),
        },
        None => vtxo.to_ark_address(),
    };

    println!("Settlement destination address: {}", to_address);

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