use std::{net::IpAddr, path::PathBuf, str::FromStr};

use anyhow::{bail, Context, Result};
use rocket::figment::{
    providers::{Format, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

pub const CONFIG_FILENAME: &str = "teepod.toml";
pub const SYSTEM_CONFIG_FILENAME: &str = "/etc/teepod/teepod.toml";
pub const DEFAULT_CONFIG: &str = include_str!("../teepod.toml");

pub fn load_config_figment(config_file: Option<&str>) -> Figment {
    let leaf_config = match config_file {
        Some(path) => Toml::file(path),
        None => Toml::file(CONFIG_FILENAME),
    };
    Figment::from(rocket::Config::default())
        .merge(Toml::string(DEFAULT_CONFIG))
        .merge(Toml::file(SYSTEM_CONFIG_FILENAME))
        .merge(leaf_config)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

impl FromStr for Protocol {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "tcp" => Protocol::Tcp,
            "udp" => Protocol::Udp,
            _ => bail!("Invalid protocol: {s}"),
        })
    }
}

impl Protocol {
    pub fn as_str(&self) -> &str {
        match self {
            Protocol::Tcp => "tcp",
            Protocol::Udp => "udp",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PortRange {
    pub protocol: Protocol,
    pub from: u16,
    pub to: u16,
}

impl PortRange {
    pub fn contains(&self, protocol: &str, port: u16) -> bool {
        self.protocol.as_str() == protocol && port >= self.from && port <= self.to
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PortMappingConfig {
    pub enabled: bool,
    pub address: IpAddr,
    pub range: Vec<PortRange>,
}

impl PortMappingConfig {
    pub fn is_allowed(&self, protocol: &str, port: u16) -> bool {
        if !self.enabled {
            return false;
        }
        self.range.iter().any(|r| r.contains(protocol, port))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CvmConfig {
    pub ca_cert: PathBuf,
    pub tmp_ca_cert: PathBuf,
    pub tmp_ca_key: PathBuf,
    /// The URL of the KMS server
    pub kms_url: String,
    /// The URL of the TProxy server
    pub tproxy_url: String,
    /// The maximum disk size in GB
    pub max_disk_size: u32,
    /// The start of the CID pool that allocates CIDs to VMs
    pub cid_start: u32,
    /// The size of the CID pool that allocates CIDs to VMs
    pub cid_pool_size: u32,
    /// Port mapping configuration
    pub port_mapping: PortMappingConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayConfig {
    pub base_domain: String,
    pub port: u16,
    pub tappd_port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub image_path: PathBuf,
    #[serde(default)]
    pub run_path: PathBuf,
    #[serde(default)]
    pub qemu_path: PathBuf,
    /// The URL of the KMS server
    pub kms_url: String,

    /// CVM configuration
    pub cvm: CvmConfig,
    /// Gateway configuration
    pub gateway: GatewayConfig,
}

impl Config {
    pub fn extract_or_default(figment: &Figment) -> Result<Self> {
        let mut me: Self = figment.extract()?;
        {
            let home = dirs::home_dir().context("Failed to get home directory")?;
            let app_home = home.join(".teepod");
            if me.image_path == PathBuf::default() {
                me.image_path = app_home.join("image");
            }
            if me.run_path == PathBuf::default() {
                me.run_path = app_home.join("vm");
            }
            if me.qemu_path == PathBuf::default() {
                let cpu_arch = std::env::consts::ARCH;
                let qemu_path = which::which(format!("qemu-system-{}", cpu_arch))
                    .context("Failed to find qemu-system-x86_64")?;
                me.qemu_path = qemu_path;
            }
        }
        Ok(me)
    }
}
