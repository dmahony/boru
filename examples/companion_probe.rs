//! Disposable companion protocol probe.
//! Invitation material is read from a protected file or stdin, never argv.

use std::{path::PathBuf, time::Duration};

use boru_core::companion_protocol::{
    CompanionInvitation, CompanionRequest, CompanionResponse, COMPANION_ALPN,
};
use iroh::{endpoint::presets, Endpoint, EndpointAddr, SecretKey};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProbeState {
    registration_id: Option<Vec<u8>>,
    device_id: Vec<u8>,
    grant_revision: i64,
    unresolved_operation_ids: Vec<String>,
    sync_cursors: Vec<String>,
}

async fn round_trip(
    endpoint: &Endpoint,
    addr: &EndpointAddr,
    request: &CompanionRequest,
) -> anyhow::Result<CompanionResponse> {
    let connection = endpoint.connect(addr.clone(), COMPANION_ALPN).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    let bytes = serde_json::to_vec(request)?;
    send.write_u32(bytes.len() as u32).await?;
    send.write_all(&bytes).await?;
    let length = recv.read_u32().await? as usize;
    anyhow::ensure!(length <= 64 * 1024, "response frame too large");
    let mut response = vec![0; length];
    recv.read_exact(&mut response).await?;
    Ok(serde_json::from_slice(&response)?)
}

fn read_invitation(path: Option<PathBuf>) -> anyhow::Result<String> {
    if let Some(path) = path {
        return Ok(std::fs::read_to_string(path)?.trim().to_owned());
    }
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    anyhow::ensure!(!input.trim().is_empty(), "invitation stdin is empty");
    Ok(input.trim().to_owned())
}

fn state_path() -> PathBuf {
    std::env::var_os("BORU_PROBE_STATE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("boru-companion-probe.json"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut invitation_file = None;
    let mut disconnect_after = false;
    let mut delay_ms = 0u64;
    let mut restart = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--invitation-file" => invitation_file = args.next().map(PathBuf::from),
            "--disconnect-after" => disconnect_after = true,
            "--delay-ms" => {
                delay_ms = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("missing delay"))?
                    .parse()?
            }
            "--restart" => restart = true,
            "--help" => {
                println!("Read invitation from stdin (or --invitation-file PATH); optional hooks: --disconnect-after --delay-ms N --restart");
                return Ok(());
            }
            other => {
                anyhow::bail!("unknown option {other}; invitation secrets are not accepted on argv")
            }
        }
    }

    let invitation_wire = read_invitation(invitation_file)?;
    let invitation = CompanionInvitation::decode(&invitation_wire)
        .map_err(|_| anyhow::anyhow!("invalid invitation"))?;
    let secret = SecretKey::generate();
    let endpoint = Endpoint::bind(presets::Minimal).await?;
    let addr = EndpointAddr::new(invitation.host_endpoint_id.parse()?);
    let path = state_path();
    let mut state: ProbeState = if restart {
        ProbeState::default()
    } else {
        std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    };
    state.device_id = secret.public().as_bytes().to_vec();

    let hello = round_trip(
        &endpoint,
        &addr,
        &CompanionRequest::Hello {
            versions: vec![1],
            capabilities: vec!["pairing".into(), "host_status".into()],
        },
    )
    .await?;
    println!("hello={hello:?}");
    if delay_ms != 0 {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    let begun = round_trip(
        &endpoint,
        &addr,
        &CompanionRequest::PairBegin {
            invitation: invitation_wire,
            device_name: "disposable-probe".into(),
        },
    )
    .await?;
    println!("pair_begin={begun:?}");
    let invitation_id = match begun {
        CompanionResponse::PairStarted { invitation_id, .. } => invitation_id,
        response => anyhow::bail!("pairing did not start: {response:?}"),
    };
    state.registration_id = Some(invitation_id.clone());
    state.sync_cursors.push("pair-status:0".into());
    std::fs::write(&path, serde_json::to_vec_pretty(&state)?)?;
    let status = round_trip(
        &endpoint,
        &addr,
        &CompanionRequest::PairStatus {
            invitation_id: invitation_id.clone(),
        },
    )
    .await?;
    println!("pair_status={status:?}");
    if let Some(registration_id) = state.registration_id.clone() {
        let host = round_trip(
            &endpoint,
            &addr,
            &CompanionRequest::HostStatus {
                request_id: "probe-host-status-1".into(),
                registration_id,
                device_id: state.device_id.clone(),
                grant_revision: state.grant_revision,
            },
        )
        .await?;
        println!("host_status={host:?}");
    }
    if disconnect_after {
        endpoint.close().await;
        return Ok(());
    }
    endpoint.close().await;
    Ok(())
}
