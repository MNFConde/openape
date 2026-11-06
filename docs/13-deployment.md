---
type: architecture
status: active
summary: "部署形态与网关边界：本机、独立服务、车载盒子三形态，daemon 对外四条契约、前端三条纪律与 nginx/Caddy 反代示例"
updated: 2026-11-05
---

# 部署形态与网关边界

daemon 只管硬件与会话，**远程接入、认证与多用户一律交给前置网关**。部署了网关就有远程能力，没部署就只有本地——能力按部署组合，代码零分叉（[decisions.md](decisions.md#d-22-远程接入由前置网关承担)）。

## 三种形态

| 形态 | 组成 | 安全边界 | 适用 |
|---|---|---|---|
| 本机单机 | daemon + 桌面应用或浏览器 | 回环，仅本机可达 | 台架旁单人作业 |
| 独立服务 | daemon + 前置网关 | 网关做 TLS 终结、认证、方法级 ACL | 办公室多人看同一台 ECU |
| 车载盒子 | 同上，跑在随车硬件上 | 同上，证书走离线自签 | 实车路试 |

三形态用的是同一份 daemon 二进制和同一份前端产物，差别只在启动参数与前置代理。

## daemon 对外契约

daemon 承诺且仅承诺以下四条，任何反向代理只要满足这四条就能接：

1. **明文 HTTP/1.1**，不做 TLS、不做认证。能连上 daemon 就等于有全部权限，因此**永不直接暴露到非可信网络**。
2. **默认只绑 `127.0.0.1`**，靠 `--listen` 显式开放（[decisions.md](decisions.md#d-22-远程接入由前置网关承担)）。形态由这一个事实决定：绑回环即本机形态，绑 `0.0.0.0` 或内网地址即服务形态，此时必须部署网关。
3. **静态页与 gRPC 同端口同前缀**——这是 [decisions.md](decisions.md#d-05-api-形态) 的单端口约束，跨端口会让 `CorsLayer` 覆盖不到预检，浏览器侧必挂。
4. **`GET /healthz` 返回 200**，供上游探活。

## 前端三条纪律

反向代理换谁都不影响 daemon，真正会出问题的是这三条：

**一、API 地址不写死。** grpc-web client 用同源相对路径（空 baseUrl 或 `location.origin`），不硬编码 `http://localhost:8080`。写死了反代后就是跨域，而 tonic 的 CORS 预检会在路由前返回 400。

**二、只支持部署在根路径。** Vite 的 `base` 保持 `/`，不支持 `/openape/` 这类子路径挂载——那要前端所有资源路径与 gRPC 前缀同步改，是纯粹的麻烦来源。

**三、长流的刷新依赖代理配合。** server-streaming 被上游缓冲时，现象是「页面能开、gRPC 调用也成功、但曲线不动」。配置侧见下节，这是部署时最容易漏的一处。

## 网关配置示例

### nginx

nginx 默认 `proxy_buffering on`，必须显式关掉：

```nginx
server {
    listen 8443 ssl;
    server_ssl_certificate     /etc/openape/server.crt;
    server_ssl_certificate_key /etc/openape/server.key;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_buffering off;
        proxy_read_timeout 3600s;
        proxy_set_header Connection "";
    }

    location = /healthz {
        proxy_pass http://127.0.0.1:8080/healthz;
        access_log off;
    }
}
```

### Caddy

Caddy 的 `flush_interval` 默认即为 `-1`（每次写出立即刷），此处显式写出是为了表达意图——与 nginx 的默认行为正好相反：

```
:8443 {
	tls internal

	reverse_proxy 127.0.0.1:8080 {
		flush_interval -1
	}
}
```

`tls internal` 用内置 CA 自动签发与轮换，全程离线——车载盒子没有公网时这是省事的那条路。nginx 侧等价做法是手动 `openssl req` 签一张自签证书并处理客户端信任。

两者的取舍：nginx 打包更小（静态 3–10MB）、生态更熟；Caddy 是静态单文件（~40MB）、配置更短、离线 TLS 免手工。资源极紧的盒子选前者，其余选后者。

## 认证与多用户不在 daemon 内实现

gRPC 的方法名就在 URL 路径里（`/openape.Session/Write`、`/openape.Session/Observe`），所以**方法级 ACL 可以由网关的路径规则完成**——只读用户放行观测类路径、拒绝写入类。daemon 侧不需要预留任何认证接口。

## 自研精简网关的触发条件

反代是成熟领域，自研要重新解决分块流式转发、`Connection` 头、长连接超时、慢客户端背压、TLS 终结——这些 nginx 与 Caddy 已经踩平了十年，收益只是省几十 MB。

**触发条件**（满足其一才做）：目标盒子放不下 40MB 静态二进制；或需要深度特化，例如 ACL 直接读取本项目工程配置、内置带 ECU 语义的健康检查。届时的做法是用 workspace 已有的 axum + tower 出一个 `gateway` crate，静态链接量级 3–5MB。
