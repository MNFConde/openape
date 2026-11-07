//! 契约验收（D-20）：只用公开 API 走通"编一条 CRO → 解一段响应"。
//!
//! 跑法：`cargo run -p openape-xcp-proto --example craft-a-cro`

fn main() {
    // 1. 编码一条 SET_MTA（0xF6）的 CRO：地址 0x1000，无地址扩展。
    let command = openape_xcp_proto::Command::SetMta {
        address_extension: 0,
        address: 0x1000,
    };
    let cro = match command.encode() {
        Ok(frame) => frame,
        Err(error) => {
            eprintln!("编码失败：{error}");
            std::process::exit(1);
        }
    };
    println!("SET_MTA CRO = {:02X?}", cro);

    // 2. 解一段正响应：从站对 SET_MTA 回空正响应（PID 0xFF）。
    let response = [0xFF];
    match openape_xcp_proto::Response::decode(&response) {
        Ok(openape_xcp_proto::Response::Ok { payload }) => {
            println!("正响应，参数 {} 字节", payload.len());
        }
        Ok(other) => println!("别的响应形态：{other:?}"),
        Err(error) => {
            eprintln!("解码失败：{error}");
            std::process::exit(1);
        }
    }
}
