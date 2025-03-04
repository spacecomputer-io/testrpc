use crate::common::TestrpcError;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::{collections::HashMap, fmt::Display, str::FromStr};

/// Adapter to use for the test flow.
/// The adapter is responsible for providing the actual implementation of the test flow for sending rpcs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Adapter {
    Hotshot,
    Libp2p, // TODO: Implement libp2p adapter
}

impl FromStr for Adapter {
    type Err = TestrpcError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "hotshot" => Ok(Adapter::Hotshot),
            "libp2p" => Ok(Adapter::Libp2p),
            _ => Err(TestrpcError::UnsupportedAdapter(s.to_string())),
        }
    }
}

impl Display for Adapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Adapter::Hotshot => "hotshot",
                Adapter::Libp2p => "libp2p",
            }
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    /// Interval between rounds in milliseconds
    pub interval: u64,
    /// Number of iterations to run, will run indefinitely if None
    pub iterations: Option<usize>,
    /// Number of expected nodes
    pub num_of_nodes: Option<usize>,
    /// Protocol adapter to use (hotshot)
    pub adapter: Adapter,
    /// Reusable round templates
    pub round_templates: HashMap<String, RoundTemplate>,
    /// Arguments for the adapter
    pub args: HashMap<String, Value>,
    /// RPCs to use for sending transactions
    pub rpcs: Option<Vec<String>>,
    /// Rounds declaration
    pub rounds: Vec<Round>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RoundTemplate {
    pub txs: usize,
    pub tx_size: usize,
    pub latency: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Round {
    pub rpcs: Vec<usize>,
    pub repeat: Option<usize>,
    pub template: Option<RoundTemplate>,
    pub use_template: Option<String>,
}

impl Round {
    /// Get the template for the round
    pub fn get_template(
        &self,
        round_templates: HashMap<String, RoundTemplate>,
    ) -> Option<RoundTemplate> {
        if let Some(template) = &self.template {
            return Some(template.clone());
        }
        if let Some(template_name) = &self.use_template {
            if let Some(template) = round_templates.get(template_name) {
                return Some(template.clone());
            }
            return None;
        }
        None
    }
}

pub fn load_config(f: &str) -> Result<Config, TestrpcError> {
    let config =
        std::fs::read_to_string(f).map_err(|e| TestrpcError::LoadConfigError(e.to_string(), f.to_string()))?;
    let config: Config = serde_yaml::from_str(config.as_str())
        .map_err(|e| TestrpcError::LoadConfigError(e.to_string(), f.to_string()))?;
    Ok(config)
}

pub fn parse_config_yaml(raw_cfg_yaml: &str) -> Result<Config, TestrpcError> {
    let config: Config = serde_yaml::from_str(raw_cfg_yaml)
        .map_err(|e| TestrpcError::LoadConfigError(e.to_string(), "".to_string()))?;
    Ok(config)
}
