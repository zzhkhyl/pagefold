//! 预览面板的**内容**：文本文件的前几 KB。
//!
//! 图片不需要在这里处理——它走 `thumbnail`（同样是 shell 的缩略图处理器），
//! 面板只是把那张位图画得更大一些。
//!
//! ## 为什么不按扩展名判断「是不是文本」
//!
//! 扩展名只能覆盖常见类型：`README`、`LICENSE`、`.gitignore`、各种配置文件
//! 都没有扩展名，而它们恰恰是最想瞄一眼的东西。这里改用**内容判断**：
//! 前 8KB 里出现 NUL 就当作二进制——这条规则与 git 的做法一致，
//! 简单且几乎不会误判（文本文件里不该有 NUL）。

use std::path::Path;

/// 最多读多少字节。
///
/// 306px 宽的面板放不下更多：32KB 的 UTF-8 文本按 12px 字号大约 800 行，
/// 而面板最多显示二十来行。读多了只是白占内存。
pub const MAX_PREVIEW_BYTES: u64 = 32 * 1024;

/// 面板里最多保留多少行。
///
/// 超出的部分直接丢掉，并在末尾留一行省略号：面板是可滚动的，
/// 但「滚不到底的滚动条」比「明确告诉你有截断」更让人困惑。
pub const MAX_PREVIEW_LINES: usize = 200;

/// 读取文本文件的开头，用于预览。
///
/// `None` 表示「不值得预览」：空文件、读不出来、或看起来是二进制。
pub fn read_text(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() == 0 {
        return None;
    }

    // 只读开头：`fs::read` 会把整个文件读进内存，碰上一个 4GB 的日志就完了。
    let bytes = read_prefix(path, MAX_PREVIEW_BYTES)?;

    // 二进制判断只看前 8KB：真正的文本文件不会在后面突然出现 NUL，
    // 而截断处的半个 UTF-8 字符不该被当成「二进制」。
    let sniff = &bytes[..bytes.len().min(8192)];
    if sniff.contains(&0) {
        return None;
    }

    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().take(MAX_PREVIEW_LINES + 1).collect();
    let truncated = lines.len() > MAX_PREVIEW_LINES;
    if truncated {
        lines.truncate(MAX_PREVIEW_LINES);
    }

    let mut result = lines.join("\n");
    if truncated {
        result.push_str("\n…");
    }
    // 全是空白（例如只有换行）也没什么可看的。
    if result.trim().is_empty() {
        return None;
    }
    Some(result)
}

/// 读文件开头最多 `limit` 字节。
fn read_prefix(path: &Path, limit: u64) -> Option<Vec<u8>> {
    use std::io::Read as _;

    let file = std::fs::File::open(path).ok()?;
    let mut buffer = Vec::with_capacity(limit.min(64 * 1024) as usize);
    // `take` 的前提是「文件可能比 limit 大得多」：它保证最多读这么多个字节。
    file.take(limit).read_to_end(&mut buffer).ok()?;
    Some(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pagefold-preview-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    #[test]
    fn reads_text_and_keeps_the_first_lines() {
        let dir = scratch("text");
        let file = dir.join("notes.md");
        fs::write(&file, "# 标题\n\n第一行\n第二行\n").expect("写入");

        let text = read_text(&file).expect("应当能预览");
        assert!(text.starts_with("# 标题"));
        assert!(text.contains("第二行"));
        assert!(!text.contains('\r'), "换行必须统一成 \\n，否则面板里会出现空行");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn binary_content_is_rejected_by_the_nul_check() {
        let dir = scratch("binary");
        let file = dir.join("blob.bin");
        // 前几个字节像文本，但含 NUL —— 扩展名说不了什么，内容可以。
        fs::write(&file, b"header\0\x01\x02payload").expect("写入");

        assert!(read_text(&file).is_none(), "含 NUL 的内容不该被当作文本");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_extensionless_file_is_previewed_by_content() {
        let dir = scratch("noext");
        let file = dir.join("README");
        fs::write(&file, "这是一个没有扩展名的说明文件。").expect("写入");

        assert!(
            read_text(&file).is_some(),
            "没有扩展名但内容可读时必须能预览（这正是按内容判断的意义）"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_files_are_truncated_and_marked() {
        let dir = scratch("long");
        let file = dir.join("long.txt");
        let body: String = (0..MAX_PREVIEW_LINES + 50)
            .map(|index| format!("第 {index} 行\n"))
            .collect();
        fs::write(&file, body).expect("写入");

        let text = read_text(&file).expect("应当能预览");
        let lines = text.lines().count();
        assert!(
            lines <= MAX_PREVIEW_LINES + 1,
            "行数必须被截断，实际 {lines}"
        );
        assert!(text.ends_with('…'), "截断时要在末尾留一个省略号");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_files_and_missing_paths_yield_nothing() {
        let dir = scratch("empty");
        let empty = dir.join("empty.txt");
        fs::write(&empty, b"").expect("写入");

        assert!(read_text(&empty).is_none(), "空文件没有可预览内容");
        assert!(read_text(&dir.join("missing.txt")).is_none(), "不存在的路径");

        let _ = fs::remove_dir_all(&dir);
    }
}
