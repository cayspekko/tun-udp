use std::net::{SocketAddr, ToSocketAddrs};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use std::sync::Arc;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::task::spawn;
use tokio_tun::TunBuilder;
use clap::Parser;
use ipnet::Ipv4Net;

const TUN_NAME: &str = "tun0";

/// UDP tunnel application.
/// Run on server example: socat UDP-LISTEN:12345,reuseaddr TUN:192.168.255.2/24,up
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Remote Server and port <server>:<port>
    #[arg(short, long, default_value = "0.0.0.0:0")]
    remote: String,

    /// Server listen mode
    #[arg(short, long, default_value_t=false)]
    server: bool,

    /// TUN ipv4 address and netmask (optional)
    #[arg(short, long, default_value = "192.168.255.1/24")]
    ip : String,

    /// bind local ip and port (optional)
    #[arg(short, long, default_value = "0.0.0.0:0")]
    bind : String
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    println!("remote: {} ip: {}", args.remote, args.ip);
    let net: Ipv4Net = args.ip.parse().unwrap();

    let socket = UdpSocket::bind(args.bind).await.unwrap();
    let r = Arc::new(socket);
    let s = r.clone();
    println!("UDP client listening on {}", r.local_addr().unwrap());
    let remote: SocketAddr;

    let (tun_tx, mut udp_rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = channel(100);
    let (udp_tx, mut tun_rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = channel(100);

    if args.server {
        let mut buf = [0u8; 65535];
        let (nbytes, src) = r.recv_from(&mut buf).await.unwrap();
        let packet = buf[..nbytes].to_vec();
        remote = src;
        udp_tx.send(packet).await.unwrap(); 
    } else {
        remote = args.remote.to_socket_addrs().unwrap().next().unwrap();
    }

    let tun = TunBuilder::new()
        .name(TUN_NAME)
        .tap(false)
        .packet_info(true)
        .address(net.addr())
        .netmask(net.netmask())
        .up()
        .try_build()
        .unwrap();
    
    let (mut reader, mut writer) = tokio::io::split(tun);
    
    let tun_read_task = spawn({
        async move {
            let mut buf = [0u8; 65535];
            loop {
                let nbytes = reader.read(&mut buf).await.unwrap();
                // println!("reading {} bytes: {:?}", nbytes, &buf[..nbytes]);
                let packet = &buf[..nbytes];
                if tun_tx.send(packet.to_vec()).await.is_err() {
                    break;
                }
            }
        }
    });

    let tun_write_task = spawn({
        async move {
            loop {
                match tun_rx.recv().await {
                    Some(packet) => {
                        if writer.write_all(&packet).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    });

    let udp_send_task = spawn({
        async move {
            loop {
                match udp_rx.recv().await {
                    Some(packet) => {
                        if s.send_to(&packet, &remote).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    });

    let udp_recv_task = spawn({
        async move {
            let mut buf = [0u8; 65535];
            loop {
                let nbytes = match r.recv(&mut buf).await {
                    Ok(nbytes) => nbytes,
                    Err(_) => break,
                };
                let packet = buf[..nbytes].to_vec();
                if udp_tx.send(packet).await.is_err() {
                    break;
                }
            }
        }
    });

    let _ = tokio::try_join!(tun_read_task, tun_write_task, udp_send_task, udp_recv_task);

}
