//! 单端口托管：同一个监听口同时服务 gRPC、grpc-web 与静态页。
//!
//! D-05 的三条约束都落在这里：
//!
//! 1. `GrpcWebLayer` —— 浏览器只会发 HTTP/1.1 的 grpc-web，服务端要能翻译；
//! 2. `accept_http1` —— 用 axum 的 `serve`（hyper 自动协商 h1/h2）而不是
//!    `tonic::transport::Server`，否则 HTTP/1.1 请求在协议层就被拒；
//! 3. Body 双向转换 —— grpc-web 的请求/响应体与 hyper 的 `Body` 不是同一个
//!    类型，交给 tonic-web 适配。
//!
//! 静态页与 API 同源是前端纪律的前提（[13-deployment]）：前端只写相对路径，
//! 才不会在浏览器里撞上跨域预检。
//!
//! [13-deployment]: ../../docs/13-deployment.md

use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::routing::get;
use openape_proto as proto;
use tonic_web::GrpcWebLayer;
use tower::ServiceBuilder;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::service::{AppState, OpenApeService};

/// gRPC 服务的路径前缀。
///
/// 不按前缀 strip 路由：gRPC 的路径就是方法全名，剥掉前缀服务端就认不出了。
const GRPC_PREFIX: &str = "/openape.v1.OpenApe";

/// 组装整个应用。
///
/// `static_dir` 不存在时不报错：API 照常可用，静态路由返回一句人话提示。
pub fn build_router(state: Arc<AppState>, static_dir: &Path) -> Router {
    let service = OpenApeService::new(state);
    let grpc = ServiceBuilder::new()
        .layer(GrpcWebLayer::new())
        .service(proto::open_ape_server::OpenApeServer::new(service));

    Router::new()
        .route("/healthz", get(healthz))
        .route_service(&format!("{GRPC_PREFIX}/{{*rest}}"), grpc)
        .fallback_service(static_files(static_dir))
        .layer(TraceLayer::new_for_http())
}

/// 存活探针。不碰硬件、不读指标，网关与运维只关心进程还在不在。
async fn healthz() -> &'static str {
    "ok\n"
}

fn static_files(static_dir: &Path) -> Router {
    if static_dir.is_dir() {
        let index = static_dir.join("index.html");
        Router::new()
            .fallback_service(ServeDir::new(static_dir).not_found_service(ServeFile::new(index)))
    } else {
        // 前端还没构建：给一句可操作的提示，而不是 404。
        let hint = format!(
            "前端未构建：{} 不存在。先在 ui/ 下运行 npm run build。\n",
            static_dir.display()
        );
        Router::new().fallback(move || {
            let hint = hint.clone();
            async move { hint }
        })
    }
}
