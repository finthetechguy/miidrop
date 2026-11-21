//! The two messages that admit a client to a network.

use super::node::{self, NODE_INFO_LEN, NodeInfo};
use super::securedata::{ETHERTYPE_EAPOL, llc};
use crate::Error;
use crate::wifi::frame::Mac;

const START: u16 = 0x0201;
const LOGOFF: u16 = 0x0202;
const REPLY_HEADER: usize = 0x18;

/// Client to host: asks to join with the client's node info.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinRequest {
    pub aid: u16,
    pub node: NodeInfo,
}

/// Host to client: the assigned node ID and the current node table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinReply {
    pub node_id: u16,
    pub client: Mac,
    pub total_nodes: u8,
    pub max_nodes: u8,
    pub nodes: Vec<NodeInfo>,
}

pub enum Message {
    Request(JoinRequest),
    Reply(JoinReply),
}

impl JoinRequest {
    pub fn to_body(&self) -> Vec<u8> {
        let mut out = llc(ETHERTYPE_EAPOL).to_vec();
        out.extend_from_slice(&START.to_be_bytes());
        out.extend_from_slice(&self.aid.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&0x0100u16.to_be_bytes());
        self.node.write(&mut out);
        out
    }
}

impl JoinReply {
    pub fn to_body(&self) -> Vec<u8> {
        let mut out = llc(ETHERTYPE_EAPOL).to_vec();
        out.extend_from_slice(&LOGOFF.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&self.node_id.to_be_bytes());
        out.extend_from_slice(&self.client);
        out.extend_from_slice(&[0; 6]);
        out.push(self.total_nodes);
        out.push(self.max_nodes);
        out.extend_from_slice(&[0; 4]);
        node::write_table(&self.nodes, &mut out);
        out
    }
}

/// Parses an EAPoL payload, the bytes after the LLC header.
pub fn parse(b: &[u8]) -> Result<Message, Error> {
    if b.len() < 2 {
        return Err(Error::Format("EAPoL message too short".into()));
    }
    match u16::from_be_bytes([b[0], b[1]]) {
        START if b.len() >= 8 + NODE_INFO_LEN => Ok(Message::Request(JoinRequest {
            aid: u16::from_be_bytes([b[2], b[3]]),
            node: NodeInfo::parse(&b[8..])?,
        })),
        LOGOFF if b.len() >= REPLY_HEADER => Ok(Message::Reply(JoinReply {
            node_id: u16::from_be_bytes([b[4], b[5]]),
            client: b[6..12].try_into().unwrap(),
            total_nodes: b[0x12],
            max_nodes: b[0x13],
            nodes: node::parse_table(&b[REPLY_HEADER..])?,
        })),
        _ => Err(Error::Format("unknown or truncated EAPoL message".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uds::securedata::parse_llc;

    #[test]
    fn request_round_trip() {
        let r = JoinRequest {
            aid: 1,
            node: NodeInfo::new(7, "PC"),
        };
        let body = r.to_body();
        assert_eq!(body.len(), 56);
        let (_, rest) = parse_llc(&body).unwrap();
        assert!(matches!(parse(rest).unwrap(), Message::Request(p) if p == r));
    }

    #[test]
    fn reply_round_trip() {
        let mut host = NodeInfo::new(1, "Host");
        host.node_id = 1;
        let mut client = NodeInfo::new(2, "PC");
        client.node_id = 2;
        let r = JoinReply {
            node_id: 2,
            client: [2; 6],
            total_nodes: 2,
            max_nodes: 2,
            nodes: vec![host, client],
        };
        let body = r.to_body();
        assert_eq!(body.len(), 672);
        let (_, rest) = parse_llc(&body).unwrap();
        assert!(matches!(parse(rest).unwrap(), Message::Reply(p) if p == r));
    }
}
