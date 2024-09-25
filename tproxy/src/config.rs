use ipnet::Ipv4Net;
use rocket::figment::{
    providers::{Format, Toml},
    Figment,
};
use serde::Deserialize;
use std::net::Ipv4Addr;

#[derive(Debug, Clone, Deserialize)]
pub struct WgConfig {
    pub public_key: String,
    pub private_key: String,
    pub listen_port: u16,
    pub ip: Ipv4Addr,
    pub client_ip_range: Ipv4Net,
    pub config_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProxyConfig {
    pub cert_chain: String,
    pub cert_key: String,
    pub listen_addr: Ipv4Addr,
    pub listen_port: u16,
    pub base_domain: String,
    pub target_port: u16,
    pub config_path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub wg: WgConfig,
    pub proxy: ProxyConfig,
}

pub const CONFIG_FILENAME: &str = "tproxy.toml";
pub const SYSTEM_CONFIG_FILENAME: &str = "/etc/tproxy/tproxy.toml";
pub const DEFAULT_CONFIG: &str = include_str!("../tproxy.toml");

pub fn load_config_figment() -> Figment {
    Figment::from(rocket::Config::default())
        .merge(Toml::string(DEFAULT_CONFIG).nested())
        .merge(Toml::file(SYSTEM_CONFIG_FILENAME).nested())
        .merge(Toml::file(CONFIG_FILENAME).nested())
}
