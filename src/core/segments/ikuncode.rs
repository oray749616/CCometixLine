use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentId};
use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct StatResponse {
    data: StatData,
    success: bool,
}

#[derive(Debug, Deserialize)]
struct StatData {
    quota: i64,
}

#[derive(Debug, Deserialize)]
struct UserResponse {
    data: UserData,
    success: bool,
}

#[derive(Debug, Deserialize)]
struct UserData {
    quota: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct IkunCodeCache {
    cost: f64,
    balance: f64,
    cached_at: String,
}

#[derive(Default)]
pub struct IkunCodeSegment;

impl IkunCodeSegment {
    pub fn new() -> Self {
        Self
    }

    fn get_cache_path() -> Option<std::path::PathBuf> {
        let home = dirs::home_dir()?;
        Some(home.join(".claude").join("ccline").join(".ikuncode_cache.json"))
    }

    fn load_cache(&self) -> Option<IkunCodeCache> {
        let cache_path = Self::get_cache_path()?;
        let content = std::fs::read_to_string(&cache_path).ok()?;
        serde_json::from_str(&content).ok()
    }

    fn save_cache(&self, cache: &IkunCodeCache) {
        if let Some(cache_path) = Self::get_cache_path() {
            if let Some(parent) = cache_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(cache) {
                let _ = std::fs::write(&cache_path, json);
            }
        }
    }

    fn is_cache_valid(&self, cache: &IkunCodeCache, cache_duration: u64) -> bool {
        if let Ok(cached_at) = DateTime::parse_from_rfc3339(&cache.cached_at) {
            let elapsed = Utc::now().signed_duration_since(cached_at.with_timezone(&Utc));
            elapsed.num_seconds() < cache_duration as i64
        } else {
            false
        }
    }

    fn fetch_data(&self, user_token: &str, user_id: &str, timeout_secs: u64) -> Option<(f64, f64)> {
        let agent = ureq::AgentBuilder::new().build();
        let timeout = std::time::Duration::from_secs(timeout_secs);

        // Fetch daily cost
        let today = Local::now().date_naive();
        let start = Local
            .from_local_datetime(&today.and_hms_opt(0, 0, 0)?)
            .single()?;
        let end = start + Duration::days(1);

        let stat_url = format!(
            "https://api.ikuncode.cc/api/log/self/stat?start_timestamp={}&end_timestamp={}&type=2",
            start.timestamp(),
            end.timestamp()
        );

        let cost = agent
            .get(&stat_url)
            .set("Authorization", &format!("Bearer {}", user_token))
            .set("New-Api-User", user_id)
            .timeout(timeout)
            .call()
            .ok()
            .and_then(|r| r.into_json::<StatResponse>().ok())
            .filter(|r| r.success)
            .map(|r| r.data.quota as f64 / 500000.0)
            .unwrap_or(0.0);

        // Fetch balance
        let balance = agent
            .get("https://api.ikuncode.cc/api/user/self")
            .set("Authorization", &format!("Bearer {}", user_token))
            .set("New-Api-User", user_id)
            .timeout(timeout)
            .call()
            .ok()
            .and_then(|r| r.into_json::<UserResponse>().ok())
            .filter(|r| r.success)
            .map(|r| r.data.quota as f64 / 500000.0)
            .unwrap_or(0.0);

        Some((cost, balance))
    }
}

impl Segment for IkunCodeSegment {
    fn collect(&self, _input: &InputData) -> Option<SegmentData> {
        let config = crate::config::Config::load().ok()?;

        if config.user_token.is_empty() || config.user_id.is_empty() {
            return Some(SegmentData {
                primary: "ikuncode 本日消费$- 余额$-".to_string(),
                secondary: String::new(),
                metadata: HashMap::new(),
            });
        }

        let segment_config = config
            .segments
            .iter()
            .find(|s| s.id == SegmentId::IkunCode);

        let timeout = segment_config
            .and_then(|sc| sc.options.get("timeout"))
            .and_then(|v| v.as_u64())
            .unwrap_or(5);

        let cache_duration = segment_config
            .and_then(|sc| sc.options.get("cache_duration"))
            .and_then(|v| v.as_u64())
            .unwrap_or(180);

        let cached_data = self.load_cache();
        let use_cached = cached_data
            .as_ref()
            .map(|c| self.is_cache_valid(c, cache_duration))
            .unwrap_or(false);

        let (cost, balance) = if use_cached {
            let cache = cached_data.unwrap();
            (cache.cost, cache.balance)
        } else {
            match self.fetch_data(&config.user_token, &config.user_id, timeout) {
                Some((cost, balance)) => {
                    self.save_cache(&IkunCodeCache {
                        cost,
                        balance,
                        cached_at: Utc::now().to_rfc3339(),
                    });
                    (cost, balance)
                }
                None => cached_data.map(|c| (c.cost, c.balance))?,
            }
        };

        let primary = format!("ikuncode 本日消费 ${:.2} 余额 ${:.2}", cost, balance);

        Some(SegmentData {
            primary,
            secondary: String::new(),
            metadata: HashMap::new(),
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::IkunCode
    }
}
