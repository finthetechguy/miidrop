use std::collections::HashMap;
use std::path::Path;

use miidrop::keys::{Key, Keys};
use miidrop::radio::pcap::{LINKTYPE_IEEE802_11, LINKTYPE_RADIOTAP, Pcap};
use miidrop::uds::eapol::{self, Message};
use miidrop::uds::securedata::{ETHERTYPE_EAPOL, ETHERTYPE_SECURE, Packet, parse_llc};
use miidrop::uds::{MII_MAKER, NetworkInfo, key::session_key};
use miidrop::wifi::frame::{Header, Mac, TYPE_DATA};
use miidrop::wifi::{ccmp, radiotap};
use serde_json::{Value, json};

use crate::hex;

fn mac(m: &Mac) -> String {
    m.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn message(body: &[u8]) -> Value {
    match parse_llc(body) {
        Ok((ETHERTYPE_EAPOL, rest)) => match eapol::parse(rest) {
            Ok(Message::Request(r)) => json!({
                "join_request": { "aid": r.aid, "console_id": format!("{:016x}", r.node.console_id), "name": r.node.name() }
            }),
            Ok(Message::Reply(r)) => json!({
                "join_reply": {
                    "node_id": r.node_id, "client": mac(&r.client), "total": r.total_nodes, "max": r.max_nodes,
                    "nodes": r.nodes.iter().map(|n| json!([n.node_id, n.name()])).collect::<Vec<_>>(),
                }
            }),
            Err(e) => json!({ "eapol_error": e.to_string(), "hex": hex(rest) }),
        },
        Ok((ETHERTYPE_SECURE, rest)) => match Packet::parse(rest) {
            Ok(p) => json!({
                "channel": p.channel, "seq": p.seq, "src": p.src, "dst": p.dst, "len": p.payload.len(),
                "head": hex(&p.payload[..p.payload.len().min(48)]),
            }),
            Err(e) => json!({ "uds_error": e.to_string(), "hex": hex(rest) }),
        },
        Ok((other, _)) => json!({ "ethertype": other }),
        Err(e) => json!({ "error": e.to_string() }),
    }
}

pub fn run(path: &Path, keys: &Keys) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let pcap = Pcap::new(&data).map_err(|e| e.to_string())?;
    let linktype = pcap.linktype;
    if linktype != LINKTYPE_RADIOTAP && linktype != LINKTYPE_IEEE802_11 {
        return Err(format!("unsupported link type {linktype}"));
    }
    let mut networks: HashMap<Mac, (NetworkInfo, Key)> = HashMap::new();
    let (mut verified, mut failed) = (0u32, 0u32);
    for packet in pcap {
        let packet = packet.map_err(|e| e.to_string())?;
        let frame = if linktype == LINKTYPE_RADIOTAP {
            match radiotap::parse(packet.data) {
                Ok((info, f)) if !info.bad_fcs => f,
                _ => continue,
            }
        } else {
            packet.data
        };
        let t = packet.timestamp_us as f64 / 1e6;
        if let Some(info) = NetworkInfo::from_beacon(frame) {
            if !info.matches(&MII_MAKER) {
                continue;
            }
            let known = networks
                .get(&info.host)
                .map(|(n, _)| (n.network_id, n.total_nodes, n.appdata.clone()));
            if known != Some((info.network_id, info.total_nodes, info.appdata.clone())) {
                let names = keys
                    .beacon
                    .and_then(|k| info.nodes(&k))
                    .map(|n| n.iter().map(|n| n.name()).collect::<Vec<_>>());
                println!(
                    "{}",
                    json!({
                        "t": t, "network": format!("{:08x}", info.network_id), "host": mac(&info.host),
                        "channel": info.channel, "nodes": [info.total_nodes, info.max_nodes],
                        "appdata": hex(&info.appdata), "names": names,
                    })
                );
                let key = session_key(&keys.session, &MII_MAKER, info.network_id, &info.host);
                networks.insert(info.host, (info, key));
            }
            continue;
        }
        let Ok(header) = Header::parse(frame) else {
            continue;
        };
        if header.ftype() != TYPE_DATA || !header.protected() {
            continue;
        }
        let Some((_, key)) = networks.get(&header.bssid()) else {
            continue;
        };
        match ccmp::decrypt(key, frame) {
            Ok((pn, body)) => {
                verified += 1;
                println!(
                    "{}",
                    json!({ "t": t, "from": mac(&header.addr2), "to": mac(&header.addr1), "pn": pn, "msg": message(&body) })
                );
            }
            Err(_) => failed += 1,
        }
    }
    eprintln!("{verified} frames authenticated, {failed} failed");
    Ok(())
}
