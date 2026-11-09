//! 出故障时给用户看的那一页。
//!
//! 壳没有 daemon 就没内容可显示（界面是 daemon 发的静态页），所以失败的
//! 情形只能自己画：一条消息、日志在哪、下一步做什么。用 `data:` URL 而不是
//! 往安装目录塞文件——壳不该为了报错写磁盘，也不该依赖 tauri 的自定义协议名。

use std::path::Path;

/// 拼一个可交给 WebView 的 `data:` 页面。
pub fn error_page(message: &str, log: &Path) -> String {
    let body = format!(
        "<html><head><meta charset=\"utf-8\"><title>OpenAPE 没能启动</title>\
         <style>body{{font:15px/1.7 system-ui,sans-serif;margin:0;padding:40px;background:#1f2328;color:#e6e8ea}}\
         h1{{font-size:19px;margin:0 0 14px}}code{{background:#2b3138;padding:2px 6px;border-radius:4px}}\
         p{{margin:6px 0}}</style></head><body>\
         <h1>OpenAPE 没能启动</h1>\
         <p>{}</p>\
         <p>daemon 日志：<code>{}</code></p>\
         <p>也可以手动起一个：<code>openaped --listen 127.0.0.1:47711</code>，\
         再用浏览器打开同一地址。</p>\
         </body></html>",
        escape(message),
        escape(&log.display().to_string())
    );
    format!("data:text/html;charset=utf-8,{}", percent_encode(&body))
}

/// HTML 转义：消息里可能带路径里的引号或 `<`。
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// 百分号编码。`data:` URL 里的非保留字符照留，其余按 UTF-8 字节转义。
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if keep {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn decode(input: &str) -> String {
        let bytes = input.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).expect("十六进制");
                out.push(u8::from_str_radix(hex, 16).expect("能解析"));
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).expect("编码后仍是 UTF-8")
    }

    #[test]
    fn the_page_carries_the_message_and_the_log_path() {
        let page = error_page(
            "daemon 起不来",
            &PathBuf::from(r"C:\Users\Admin\AppData\Local\openape\logs\daemon.log"),
        );
        let html = decode(
            page.strip_prefix("data:text/html;charset=utf-8,")
                .expect("带前缀"),
        );

        assert!(html.contains("daemon 起不来"));
        assert!(html.contains(r"C:\Users\Admin\AppData\Local\openape\logs\daemon.log"));
    }

    #[test]
    fn markup_in_the_message_is_escaped() {
        let page = error_page("<script>alert(1)</script>", Path::new("/tmp/daemon.log"));
        let html = decode(
            page.strip_prefix("data:text/html;charset=utf-8,")
                .expect("带前缀"),
        );

        assert!(!html.contains("<script>"), "消息里的标签不能原样进页面");
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn the_result_is_a_usable_data_url() {
        let page = error_page("端口被占", Path::new("/tmp/daemon.log"));
        assert!(page.starts_with("data:text/html;charset=utf-8,"));
        assert!(
            page.bytes().all(|byte| byte.is_ascii()),
            "URL 里不该出现非 ASCII 字节"
        );
        assert!(!page.contains(' '), "空格按 %20 转义，免得被某些解析器截断");
    }
}
