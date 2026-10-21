//! 凭证验证面的频率风控：按来源 IP 与按租户两条战线的失败计数 + 冷却。
//! 进程内存实现（重启清零）；冷却期惰性过期，无后台清扫任务。
//!
//! 策略（协议「风控」一节）：
//! - gate/login 与控制面鉴权失败按 IP：窗口内失败超阈值 → 固定冷却
//! - login 另按租户：失败超阈值 → 整池冷却（防分布式 IP 撞同一租户的 key），
//!   冷却时长随连续触发指数增长（封顶 24h），一次成功登录清零

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 失败计数窗口
const WINDOW: Duration = Duration::from_secs(10 * 60);
/// 单 IP 在窗口内允许的最大失败次数（gate/login/控制面共用）
const IP_MAX_FAILURES: u32 = 30;
/// 单 IP 冷却时长（固定）
const IP_COOLDOWN: Duration = Duration::from_secs(15 * 60);
/// 单租户在窗口内允许的最大登录失败次数（超过进入池冷却）
const TENANT_MAX_FAILURES: u32 = 20;
/// 租户冷却起始时长（连续触发指数增长，封顶 24h）
const TENANT_COOLDOWN_MIN: Duration = Duration::from_secs(15 * 60);
const TENANT_COOLDOWN_MAX: Duration = Duration::from_secs(24 * 60 * 60);

struct Bucket {
    /// 窗口内失败计数
    failures: u32,
    window_start: Instant,
    /// 冷却截止（None = 未冷却）
    cooldown_until: Option<Instant>,
    /// 下一次冷却时长（租户桶连续触发时指数增长）
    cooldown_level: Duration,
}

impl Bucket {
    fn new(base_cooldown: Duration) -> Bucket {
        Bucket {
            failures: 0,
            window_start: Instant::now(),
            cooldown_until: None,
            cooldown_level: base_cooldown,
        }
    }

    /// 剩余冷却秒数（未在冷却返回 None）
    fn cooling(&self) -> Option<u64> {
        self.cooldown_until.and_then(|until| {
            let now = Instant::now();
            (until > now).then(|| (until - now).as_secs())
        })
    }

    /// 记录一次失败；返回因本次失败而进入的冷却秒数（未触发冷却返回 None）
    fn fail(&mut self, max: u32, escalate: bool) -> Option<u64> {
        let now = Instant::now();
        if now - self.window_start > WINDOW {
            self.failures = 0;
            self.window_start = now;
        }
        self.failures += 1;
        if self.failures < max {
            return None;
        }
        self.cooldown_until = Some(now + self.cooldown_level);
        let applied = self.cooldown_level.as_secs();
        if escalate {
            self.cooldown_level = (self.cooldown_level * 2).min(TENANT_COOLDOWN_MAX);
        }
        self.failures = 0;
        self.window_start = now;
        Some(applied)
    }

    fn success(&mut self, base_cooldown: Duration) {
        self.failures = 0;
        self.cooldown_until = None;
        self.cooldown_level = base_cooldown;
    }
}

pub struct RateLimiter {
    ip: Mutex<HashMap<IpAddr, Bucket>>,
    tenant: Mutex<HashMap<String, Bucket>>,
}

impl Default for RateLimiter {
    fn default() -> RateLimiter {
        RateLimiter {
            ip: Mutex::new(HashMap::new()),
            tenant: Mutex::new(HashMap::new()),
        }
    }
}

impl RateLimiter {
    pub fn new() -> RateLimiter {
        RateLimiter::default()
    }

    /// 检查 IP 是否在冷却中；是 → 剩余秒数
    pub fn check_ip(&self, ip: IpAddr) -> Option<u64> {
        self.ip.lock().unwrap().get(&ip).and_then(Bucket::cooling)
    }

    /// 记录一次 IP 失败（达到阈值自动进入冷却）
    pub fn fail_ip(&self, ip: IpAddr) {
        let mut map = self.ip.lock().unwrap();
        let bucket = map.entry(ip).or_insert_with(|| Bucket::new(IP_COOLDOWN));
        if let Some(secs) = bucket.fail(IP_MAX_FAILURES, false) {
            tracing::warn!("来源 IP {ip} 凭证验证失败过多，冷却 {} 分钟", secs / 60);
        }
    }

    /// 检查租户（池）是否在冷却中
    pub fn check_tenant(&self, tenant_id: &str) -> Option<u64> {
        self.tenant
            .lock()
            .unwrap()
            .get(tenant_id)
            .and_then(Bucket::cooling)
    }

    /// 记录一次租户登录失败（达到阈值整池冷却，连续触发冷却翻倍）
    pub fn fail_tenant(&self, tenant_id: &str) {
        let mut map = self.tenant.lock().unwrap();
        let bucket = map
            .entry(tenant_id.to_string())
            .or_insert_with(|| Bucket::new(TENANT_COOLDOWN_MIN));
        if let Some(secs) = bucket.fail(TENANT_MAX_FAILURES, true) {
            tracing::warn!("租户 {tenant_id} 登录失败过多，池冷却 {} 分钟", secs / 60);
        }
    }

    /// 登录成功：清零该租户的失败计数与冷却档
    pub fn success_tenant(&self, tenant_id: &str) {
        if let Some(bucket) = self.tenant.lock().unwrap().get_mut(tenant_id) {
            bucket.success(TENANT_COOLDOWN_MIN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_cools_down_after_threshold() {
        let limiter = RateLimiter::new();
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        assert!(limiter.check_ip(ip).is_none());
        for _ in 0..IP_MAX_FAILURES - 1 {
            limiter.fail_ip(ip);
        }
        assert!(limiter.check_ip(ip).is_none());
        limiter.fail_ip(ip);
        assert!(limiter.check_ip(ip).is_some());
    }

    #[test]
    fn tenant_success_resets_failures() {
        let limiter = RateLimiter::new();
        for _ in 0..TENANT_MAX_FAILURES - 1 {
            limiter.fail_tenant("t_1");
        }
        limiter.success_tenant("t_1");
        // 清零后再到阈值才冷却
        for _ in 0..TENANT_MAX_FAILURES - 1 {
            limiter.fail_tenant("t_1");
        }
        assert!(limiter.check_tenant("t_1").is_none());
        limiter.fail_tenant("t_1");
        assert!(limiter.check_tenant("t_1").is_some());
    }

    #[test]
    fn tenant_cooldown_escalates() {
        let limiter = RateLimiter::new();
        for _ in 0..TENANT_MAX_FAILURES {
            limiter.fail_tenant("t_1");
        }
        let first = limiter.check_tenant("t_1").unwrap();
        assert!(
            first <= TENANT_COOLDOWN_MIN.as_secs() && first > TENANT_COOLDOWN_MIN.as_secs() - 5
        );
        // 冷却结束后再次打满：冷却档翻倍
        let mut map = limiter.tenant.lock().unwrap();
        let bucket = map.get_mut("t_1").unwrap();
        bucket.cooldown_until = None;
        assert_eq!(bucket.cooldown_level, TENANT_COOLDOWN_MIN * 2);
    }
}
