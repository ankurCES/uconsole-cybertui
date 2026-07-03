//! wifi-radar binary entrypoint.
//!
//! Parses command-line args, initialises tracing, then delegates to
//! [`wifi_radar::run::run_with`]. This mirrors `cyberdeck-web`'s
//! lib/bin split so the long-lived async function is testable from the
//! library API.

use std::net::SocketAddr;
use std::path::PathBuf;

use wifi_radar::run::{RunConfig, DEFAULT_STATIC_DIR, DEFAULT_TAGS_PATH};

const USAGE: &str = "wifi-radar [OPTIONS]

Options:
  --bind <ADDR>       Bind address (default: 127.0.0.1:8743)
  --dev               Force the dev-mode synthetic scanner
  --tags <PATH>       Tag DB path (default: data/tags.json)
  --static-dir <DIR>  Static asset directory (default: crates/wifi-radar/web)
  --pcap <PATH>       Read frames from a pcap file instead of scanning live
  --nexmon            Enable CSI vitals on 0.0.0.0:5500 (nexmon_csi default)
  --csi-udp <ADDR>    Listen for nexmon_csi UDP frames on this address
  --csi-pcap <PATH>   Read nexmon CSI from a pcap stream (- = stdin; use with
                      `tcpdump -i wlan0 -U -w - 'udp port 5500'`)
  --csi-rate <HZ>     Fixed CSI sample rate; omit to estimate from arrivals
  --csi-motion-threshold <F>  Presence/motion sensitivity (default: 0.15)
  -h, --help          Print this help
";

fn main() {
    let mut bind: SocketAddr = "127.0.0.1:8743".parse().unwrap();
    let mut dev_mode = false;
    let mut tags_path = PathBuf::from(DEFAULT_TAGS_PATH);
    let mut static_dir = PathBuf::from(DEFAULT_STATIC_DIR);
    let mut pcap_path: Option<PathBuf> = None;
    let mut csi_udp: Option<SocketAddr> = None;
    let mut csi_pcap: Option<PathBuf> = None;
    let mut csi_rate_hz: Option<f32> = None;
    let mut csi_motion_threshold: f32 = wifi_radar::csi::DEFAULT_MOTION_THRESHOLD;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--bind" => {
                i += 1;
                bind = args[i]
                    .parse()
                    .unwrap_or_else(|_| panic!("invalid --bind: {}", args[i]));
            }
            "--dev" => dev_mode = true,
            "--tags" => {
                i += 1;
                tags_path = PathBuf::from(&args[i]);
            }
            "--static-dir" => {
                i += 1;
                static_dir = PathBuf::from(&args[i]);
            }
            "--pcap" => {
                i += 1;
                pcap_path = Some(PathBuf::from(&args[i]));
            }
            "--nexmon" => {
                if csi_udp.is_none() {
                    csi_udp = Some("0.0.0.0:5500".parse().unwrap());
                }
            }
            "--csi-udp" => {
                i += 1;
                csi_udp = Some(
                    args[i]
                        .parse()
                        .unwrap_or_else(|_| panic!("invalid --csi-udp: {}", args[i])),
                );
            }
            "--csi-pcap" => {
                i += 1;
                csi_pcap = Some(PathBuf::from(&args[i]));
            }
            "--csi-rate" => {
                i += 1;
                csi_rate_hz = Some(
                    args[i]
                        .parse()
                        .unwrap_or_else(|_| panic!("invalid --csi-rate: {}", args[i])),
                );
            }
            "--csi-motion-threshold" => {
                i += 1;
                csi_motion_threshold = args[i]
                    .parse()
                    .unwrap_or_else(|_| panic!("invalid --csi-motion-threshold: {}", args[i]));
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            other => {
                eprintln!("unknown flag: {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    init_tracing();

    let cfg = RunConfig {
        bind,
        dev_mode,
        tags_path,
        static_dir,
        pcap_path,
        csi_udp,
        csi_pcap,
        csi_rate_hz,
        csi_motion_threshold,
    };

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    if let Err(e) = rt.block_on(wifi_radar::run::run_with(cfg)) {
        eprintln!("wifi-radar: {e:#}");
        std::process::exit(1);
    }
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,wifi_radar=info"));
    let _ = fmt().with_env_filter(filter).try_init();
}