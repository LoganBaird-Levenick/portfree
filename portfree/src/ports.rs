//! Native TCP table access. The `PortTable` trait is the backend seam.

use crate::model::{Family, RowState, TcpRow};

#[derive(Debug)]
pub struct TableError(pub String);

impl std::fmt::Display for TableError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TableError {}

pub trait PortTable {
    /// All TCP rows, IPv4 and IPv6, in every state. Filtering to `LISTEN` is the
    /// resolver's job so a fake table can exercise TIME_WAIT and other states.
    fn tcp_rows(&mut self) -> Result<Vec<TcpRow>, TableError>;
}

pub struct NativePortTable;

impl PortTable for NativePortTable {
    fn tcp_rows(&mut self) -> Result<Vec<TcpRow>, TableError> {
        use netstat2::{
            AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState, get_sockets_info,
        };

        let sockets = get_sockets_info(
            AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6,
            ProtocolFlags::TCP,
        )
        .map_err(|e| TableError(format!("GetExtendedTcpTable failed: {e}")))?;

        let mut rows = Vec::new();
        for socket in sockets {
            let ProtocolSocketInfo::Tcp(tcp) = socket.protocol_socket_info else {
                continue;
            };
            let family = Family::of(tcp.local_addr);
            let state = match tcp.state {
                TcpState::Listen => RowState::Listen,
                _ => RowState::Other,
            };
            rows.push(TcpRow {
                family,
                address: tcp.local_addr,
                port: tcp.local_port,
                state,
                pids: socket.associated_pids,
            });
        }
        Ok(rows)
    }
}
