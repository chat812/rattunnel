use anyhow::Result;
use std::sync::Arc;
use tokio::net::UdpSocket;

use crate::config::Config;
use crate::db::Db;

/// Parse a DNS name from a packet buffer starting at `offset`.
/// Handles label compression (pointers).
fn parse_name(buf: &[u8], offset: &mut usize) -> Option<String> {
    let mut name = String::new();
    let mut jumped_to: Option<usize> = None;
    let mut safety = 0usize;

    loop {
        safety += 1;
        if safety > 128 || *offset >= buf.len() {
            return None;
        }

        let b = buf[*offset] as usize;

        if b == 0 {
            *offset += 1;
            break;
        }

        // Compression pointer
        if (b & 0xC0) == 0xC0 {
            if *offset + 1 >= buf.len() {
                return None;
            }
            let ptr = ((b & 0x3F) << 8) | (buf[*offset + 1] as usize);
            if jumped_to.is_none() {
                jumped_to = Some(*offset + 2);
            }
            *offset = ptr;
            continue;
        }

        let label_len = b;
        *offset += 1;
        if *offset + label_len > buf.len() {
            return None;
        }
        if !name.is_empty() {
            name.push('.');
        }
        let label = std::str::from_utf8(&buf[*offset..*offset + label_len]).ok()?;
        name.push_str(label);
        *offset += label_len;
    }

    if let Some(j) = jumped_to {
        *offset = j;
    }

    Some(name)
}

/// Build a DNS response for an A query.
/// `ip = Some(addr)` → NOERROR with A record
/// `ip = None`       → NXDOMAIN, no answer
fn build_a_response(query: &[u8], ip: Option<[u8; 4]>) -> Vec<u8> {
    let mut resp = Vec::with_capacity(512);

    // Transaction ID
    resp.extend_from_slice(&query[0..2]);

    // Flags: QR=1, Opcode=0, AA=1, TC=0, RD=copy bit, RA=0
    // RCODE: 0=NOERROR, 3=NXDOMAIN
    let rd = query[2] & 0x01;
    let rcode: u8 = if ip.is_some() { 0 } else { 3 };
    resp.push(0x84 | rd); // 1000_0100
    resp.push(rcode);

    // QDCOUNT = 1
    resp.extend_from_slice(&[0x00, 0x01]);
    // ANCOUNT
    resp.push(0x00);
    resp.push(if ip.is_some() { 1 } else { 0 });
    // NSCOUNT, ARCOUNT
    resp.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

    // Echo question section: find its byte range in the query
    let q_start = 12usize;
    let mut pos = q_start;
    if pos < query.len() {
        // Skip QNAME
        loop {
            if pos >= query.len() {
                break;
            }
            let b = query[pos] as usize;
            if b == 0 {
                pos += 1;
                break;
            }
            if (b & 0xC0) == 0xC0 {
                pos += 2;
                break;
            }
            pos += 1 + b;
        }
        // Skip QTYPE + QCLASS
        pos = pos.saturating_add(4);
        if pos <= query.len() {
            resp.extend_from_slice(&query[q_start..pos]);
        }
    }

    // Answer RR
    if let Some(addr) = ip {
        // NAME: pointer back to QNAME at offset 12
        resp.extend_from_slice(&[0xC0, 0x0C]);
        // TYPE A
        resp.extend_from_slice(&[0x00, 0x01]);
        // CLASS IN
        resp.extend_from_slice(&[0x00, 0x01]);
        // TTL 60s
        resp.extend_from_slice(&[0x00, 0x00, 0x00, 0x3C]);
        // RDLENGTH 4
        resp.extend_from_slice(&[0x00, 0x04]);
        // RDATA
        resp.extend_from_slice(&addr);
    }

    resp
}

/// Return REFUSED for queries outside our zone
fn refused_response(query: &[u8]) -> Vec<u8> {
    let mut resp = query[..query.len().min(12)].to_vec();
    resp.resize(12, 0);
    resp[2] = 0x84; // QR=1 AA=1
    resp[3] = 0x05; // RCODE=5 REFUSED
    resp[6] = 0;
    resp[7] = 0; // ANCOUNT=0
    resp
}

fn handle_query(buf: &[u8], db: &Db, server_ip: &[u8; 4], domain: &str) -> Vec<u8> {
    if buf.len() < 12 {
        return vec![];
    }

    let qdcount = u16::from_be_bytes([buf[4], buf[5]]);
    if qdcount == 0 {
        return refused_response(buf);
    }

    let mut offset = 12usize;
    let name = match parse_name(buf, &mut offset) {
        Some(n) => n.to_lowercase(),
        None => return refused_response(buf),
    };

    if offset + 4 > buf.len() {
        return refused_response(buf);
    }

    let qtype = u16::from_be_bytes([buf[offset], buf[offset + 1]]);

    let suffix = format!(".{}", domain);
    let in_zone = name.ends_with(&suffix) || name == domain;
    if !in_zone {
        return refused_response(buf);
    }

    // For non-A queries in our zone, return NXDOMAIN
    if qtype != 1 {
        return build_a_response(buf, None);
    }

    // A query — look up DB
    let exists = db.subdomain_exists(&name).unwrap_or(false);
    if exists {
        build_a_response(buf, Some(*server_ip))
    } else {
        build_a_response(buf, None)
    }
}

pub async fn run_dns_server(db: Arc<Db>, cfg: Arc<Config>) -> Result<()> {
    let octets: Vec<u8> = cfg.server_ip
        .split('.')
        .filter_map(|s| s.parse().ok())
        .collect();
    if octets.len() != 4 {
        anyhow::bail!("Invalid server_ip in config: {}", cfg.server_ip);
    }
    let server_ip: [u8; 4] = [octets[0], octets[1], octets[2], octets[3]];

    let socket = UdpSocket::bind("0.0.0.0:53").await?;
    log::info!("DNS server listening on 0.0.0.0:53 (zone: {} → {})", cfg.domain, cfg.server_ip);

    let mut buf = [0u8; 512];
    loop {
        let (len, peer) = match socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                log::warn!("DNS recv error: {}", e);
                continue;
            }
        };

        let packet = buf[..len].to_vec();
        let resp = handle_query(&packet, &db, &server_ip, &cfg.domain);
        if !resp.is_empty() {
            if let Err(e) = socket.send_to(&resp, peer).await {
                log::warn!("DNS send error: {}", e);
            }
        }
    }
}
