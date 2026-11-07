//! `openaped`：常驻外壳。
//!
//! 默认只绑回环（D-22）：远程接入、认证与 ACL 由前置网关承担，daemon 自己
//! 不实现。要对外提供服务，用 `--listen 0.0.0.0:47711` 显式开放。
//!
//! 二进制里没有逻辑：解析、开会话、组路由、绑定与 serve 全在库的 `boot` 里，
//! 那样才测得动。这里只有一行转发。

#[tokio::main]
async fn main() -> Result<(), openape_diag::Fault> {
    openape_daemon::boot::run_from_env().await
}
