//! 由 `.proto` 生成 prost 类型与 tonic 客户端/服务端。
//!
//! 生成的类型同时承担两件事：gRPC 的消息，以及 CLI `--json` 的结构。为此给
//! 所有生成类型挂 serde derive——不引 pbjson，它按 proto3 JSON 映射把字段转
//! camelCase、把 int64 转字符串，等于为"符合协议"改动对外的 JSON 形状。
//!
//! 字段名不另作处理：proto 里就写 snake_case，prost 生成的 Rust 字段便是
//! snake_case，serde 默认按字段名序列化，正好与 CLI 既有输出一致。

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .compile_protos(&["proto/openape/v1/openape.proto"], &["proto"])?;

    println!("cargo:rerun-if-changed=proto/openape/v1/openape.proto");
    Ok(())
}
