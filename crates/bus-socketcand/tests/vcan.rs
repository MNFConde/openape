//! socketcand 端到端（M5-11）：真守护进程 + vcan + sim-ecu 从站，
//! master 经 socketcand 走 CONNECT 与 DAQ。
//!
//! 依赖两个外部件：
//! - `SOCKETCAND_BIN` 指向编译好的 socketcand（默认
//!   `tmp/socketcand/build/src/socketcand`，构建方式见
//!   script/scripts.md）；
//! - vcan 可用。任一缺席就跳过——CI 没有 vcan 时不红。

// 集成测试里的 expect 是断言手段：失败即测试失败，不走生产那道门。
#![allow(clippy::expect_used)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use openape_bus::Bus;
use openape_bus_socketcan::SocketCanBus;
use openape_bus_socketcan::testing::VcanGuard;
use openape_bus_socketcand::SocketcandBus;
use openape_cal_core::{Address, ByteLen};
use openape_sim_ecu::{SimEcu, SimEcuConfig};
use openape_xcp::daq::{DaqListSpec, DaqSignal};
use openape_xcp::{CanXcpMaster, XcpConfig, XcpConnection};
use openape_xcp_proto::DaqFrame;

fn socketcand_bin() -> Option<PathBuf> {
    let path = std::env::var("SOCKETCAND_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            // cargo test 的 cwd 是 crate 目录；workspace 根在两级之上。
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tmp/socketcand/build/src/socketcand")
        });
    path.is_file().then_some(path)
}

fn signal(address: u32, len: u16) -> DaqSignal {
    DaqSignal {
        address: Address::new(address),
        len: ByteLen::new(len).expect("长度非零"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn socketcand_carries_connect_and_daq_over_vcan() {
    let Some(vcan) = VcanGuard::create("cand") else {
        return;
    };
    let Some(socketcand) = socketcand_bin() else {
        eprintln!("跳过：没有 SOCKETCAND_BIN（构建方式见 script/scripts.md）");
        return;
    };

    // 起守护进程：只管给的这个 vcan，占用一个测试端口，不广播。
    // 监听显式钉回环——socketcand 的 -l 是接口名，`lo` 才是回环。
    let port = 29536;
    let mut daemon = tokio::process::Command::new(&socketcand)
        .args(["-i", vcan.name(), "-p", &port.to_string(), "-l", "lo", "-n"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("socketcand 要能启动");

    // 等它绑定端口：有界轮询而不是固定 sleep——固定时长在慢机器上假失败、
    // 在快机器上白等。
    let address: SocketAddr = format!("127.0.0.1:{port}").parse().expect("合法地址");
    let mut bound = false;
    for _ in 0..200 {
        if tokio::net::TcpStream::connect(address).await.is_ok() {
            bound = true;
            break;
        }
        assert!(
            matches!(daemon.try_wait(), Ok(None)),
            "守护进程要活着：提前退出说明端口被占或参数不对"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if !bound {
        // 超时把守护进程的 stderr 一起打出来，否则只剩一句"连不上"。
        let mut stderr = String::new();
        if let Some(mut pipe) = daemon.stderr.take() {
            use tokio::io::AsyncReadExt;
            let _ = pipe.read_to_string(&mut stderr).await;
        }
        panic!("socketcand 两秒内没绑定 {address}；stderr：{stderr}");
    }

    // 从站挂 vcan。
    let bus: Arc<dyn Bus> = Arc::new(SocketCanBus::open(vcan.name()).expect("从站总线"));
    let mut ecu = SimEcu::new(SimEcuConfig::default_ids());
    ecu.poke(0x1000, &[0x11, 0x22]);
    let slave = tokio::spawn(openape_sim_ecu::serve(ecu, bus));

    // master 经 socketcand 上总线。
    let cand = Arc::new(
        SocketcandBus::open(address, vcan.name())
            .await
            .expect("socketcand 握手要成功"),
    );
    assert_eq!(cand.backend(), "socketcand");
    assert_eq!(cand.channel(), vcan.name());
    assert!(
        !cand.capabilities().cro_padding,
        "socketcand 的帧按实际字节呈现，能力位要如实"
    );

    let mut master = CanXcpMaster::new(
        cand.clone(),
        XcpConfig::new(
            openape_bus::CanId::standard(0x600).expect("合法"),
            openape_bus::CanId::standard(0x601).expect("合法"),
        ),
    );
    let info = master.connect(0).await.expect("CONNECT 要经 socketcand 通");
    assert_ne!(info.max_cto, 0);

    // DAQ：配置后从 vcan 上出的 DTO 要绕一圈回到 master。
    let layout = master
        .configure_daq(&[DaqListSpec::new(0, 1, vec![signal(0x1000, 2)]).expect("配置合法")])
        .await
        .expect("DAQ 配置要走通");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    // sim-ecu 的内存是静态的：一条 DTO 值对上即验收（变化性已在
    // udp_eth 回环里验过）。
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!remaining.is_zero(), "三秒内要收到 DTO");
        let frame = cand.recv(remaining).await.expect("要收到 DTO");
        let Some(parsed) = DaqFrame::parse(&frame.data) else {
            continue;
        };
        let decoded: Vec<u8> = layout
            .decode(parsed.odt, parsed.payload)
            .expect("布局认识这个 ODT")
            .concat();
        assert_eq!(decoded, vec![0x11, 0x22], "DTO 的值来自 poke 的内存");
        break;
    }

    slave.abort();
    daemon.kill().await.expect("守护进程能收尾");
}
