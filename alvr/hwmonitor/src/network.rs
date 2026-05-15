//! Per-adapter network counters from `root\cimv2`.
//!
//! `Win32_PerfFormattedData_Tcpip_NetworkInterface` already pre-computes the
//! per-second rates between WMI sample intervals — we just read them.

use crate::NetSample;
use alvr_common::anyhow;
use serde::Deserialize;
use wmi::{COMLibrary, WMIConnection};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct RawNetCounter {
    name: String,
    #[serde(rename = "BytesSentPersec")]
    bytes_sent_persec: u64,
    #[serde(rename = "BytesReceivedPersec")]
    bytes_received_persec: u64,
    #[serde(rename = "PacketsSentPersec")]
    packets_sent_persec: u64,
    #[serde(rename = "PacketsReceivedPersec")]
    packets_received_persec: u64,
    packets_outbound_errors: u64,
    packets_outbound_discarded: u64,
    current_bandwidth: u64,
}

pub struct NetSource {
    conn: WMIConnection,
}

impl NetSource {
    pub fn connect(com: COMLibrary) -> anyhow::Result<Self> {
        let conn = WMIConnection::new(com)?;
        Ok(Self { conn })
    }

    pub fn read(&self) -> anyhow::Result<Vec<NetSample>> {
        let rows: Vec<RawNetCounter> = self.conn.raw_query(
            "SELECT Name, BytesSentPersec, BytesReceivedPersec, \
             PacketsSentPersec, PacketsReceivedPersec, \
             PacketsOutboundErrors, PacketsOutboundDiscarded, CurrentBandwidth \
             FROM Win32_PerfFormattedData_Tcpip_NetworkInterface",
        )?;
        Ok(rows
            .into_iter()
            .filter(|r| !is_virtual(&r.name))
            .map(|r| NetSample {
                adapter: r.name,
                bytes_sent_per_sec: r.bytes_sent_persec,
                bytes_recv_per_sec: r.bytes_received_persec,
                packets_sent_per_sec: r.packets_sent_persec,
                packets_recv_per_sec: r.packets_received_persec,
                outbound_errors: r.packets_outbound_errors,
                outbound_discarded: r.packets_outbound_discarded,
                current_bandwidth_bps: r.current_bandwidth,
            })
            .collect())
    }
}

fn is_virtual(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("loopback") || n.contains("isatap") || n.contains("teredo")
}
