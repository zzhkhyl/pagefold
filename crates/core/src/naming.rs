//! 名称规则：校验、扩展名切分、重名去重与重命名选区。
//!
//! 全部是**不碰文件系统**的纯函数，因此可以完整单测。这不是为了好看：
//! 真实的重命名 / 新建失败里，绝大多数是**名称本身不合法**或**重名**，
//! 而不是 IO 出错——这两类必须在提交给系统之前就拦住，
//! 否则用户看到的是 `Os { code: 123, kind: InvalidFilename, .. }` 这种没法读的东西。
//!
//! 判定规则照搬 Windows（资源管理器）的行为，刻意不做「更宽容」的处理：
//! 本程序的目标平台就是 Windows，宽容只会让「本地能建、拷到别处炸」。

use std::collections::HashSet;
use std::ops::Range;

/// 新建文件夹的默认名。
pub const DEFAULT_FOLDER: &str = "新建文件夹";
/// 新建文本文档的默认名。
pub const DEFAULT_TEXT_FILE: &str = "新建文本文档.txt";
/// 新建 Markdown 文档的默认名（设计稿的「新建」下拉里有这一项）。
pub const DEFAULT_MARKDOWN_FILE: &str = "新建文档.md";

/// 单个路径分量的长度上限。
///
/// 单位是 **UTF-16 码元**而不是字符数：NTFS 的限制是 255 个 UTF-16 码元，
/// 一个汉字算 1 个、一个 emoji 算 2 个。若按 `chars().count()` 判定，
/// 「255 个 emoji」这种名称会通过校验、然后在系统调用上失败。
pub const MAX_NAME_UNITS: usize = 255;

/// 重名时最多尝试到 `(MAX_SUFFIX)`。
///
/// 纯粹是防御性上限：真到这一步说明目录里堆了上万个同名项，
/// 与其在这里无限循环，不如把原样名称交给系统去报「已存在」。
const MAX_SUFFIX: usize = 9999;

/// Windows 禁止出现在名称里的字符。
const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Windows 保留的设备名。判定在主干（第一个点之前）上做，因此 `CON.txt` 同样被拦下。
const RESERVED_DEVICES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 名称不被接受的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    /// 去掉首尾空白后为空。
    Empty,
    /// 含控制字符（含 `NUL`）。
    Control,
    /// 含 [`FORBIDDEN`] 里的字符之一。
    Forbidden(char),
    /// 以点结尾。
    TrailingDot,
    /// 是 Windows 保留的设备名（`CON` / `NUL` / `COM1` …）。
    ReservedDevice,
    /// 超过 [`MAX_NAME_UNITS`]。
    TooLong,
}

impl NameError {
    /// 面向用户的说明。**不带**「名称无效：」这类前缀，由调用方决定怎么拼。
    pub fn message(self) -> String {
        match self {
            Self::Empty => String::from("名称不能为空"),
            Self::Control => String::from("名称不能包含控制字符"),
            Self::Forbidden(c) => format!("名称不能包含字符「{c}」"),
            Self::TrailingDot => String::from("名称不能以点结尾"),
            Self::ReservedDevice => String::from("该名称是 Windows 保留的设备名"),
            Self::TooLong => format!("名称过长（上限 {MAX_NAME_UNITS} 个字符，中文与 emoji 按 2 计）"),
        }
    }
}

/// 校验一个待创建 / 待重命名的名称，返回**去掉首尾空白后**的名称。
///
/// 先 trim 再校验，与设计稿的 `e.value.trim()` 一致：用户从别处拷来的名称
/// 常带一个尾随空格，而 Windows 会把结尾的空格与点**静默吃掉**——
/// 那是唯一一种「看起来成功了、名字却和输入的不一样」的输入。
pub fn validate(name: &str) -> Result<&str, NameError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(NameError::Empty);
    }
    // 控制字符先于禁用字符报：它们的展示形式是空白或方框，
    // 先报「不能包含控制字符」比报「不能包含字符『\u{1}』」有用得多。
    if trimmed.chars().any(char::is_control) {
        return Err(NameError::Control);
    }
    if let Some(found) = trimmed.chars().find(|c| FORBIDDEN.contains(c)) {
        return Err(NameError::Forbidden(found));
    }
    // trim 已经处理掉尾随空格，这里只剩点。`.` 与 `..` 也走这一支。
    if trimmed.ends_with('.') {
        return Err(NameError::TrailingDot);
    }
    if is_reserved_device(trimmed) {
        return Err(NameError::ReservedDevice);
    }
    if trimmed.encode_utf16().count() > MAX_NAME_UNITS {
        return Err(NameError::TooLong);
    }
    Ok(trimmed)
}

/// 是否为 Windows 保留设备名。
///
/// 只比对**主干**（第一个点之前）且大小写不敏感，因此 `nul.txt` 也会被拦下——
/// 与 Windows 自身的行为一致。
fn is_reserved_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    RESERVED_DEVICES
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

/// 把名称切成「主干」与「扩展名（**含**开头的点）」。
///
/// 三点与资源管理器一致：
/// - 用**最后一个**点：`archive.tar.gz` 的主干是 `archive.tar`；
/// - **开头的点不算分隔符**：`.gitignore` 没有扩展名（主干就是它自己）；
/// - 点后面为空则没有扩展名：`a.` 的主干是 `a.`。
pub fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        // `index > 0` 排除 `/.gitignore` 这种，`index + 1 < len` 排除 `a.`。
        Some(index) if index > 0 && index + 1 < name.len() => (&name[..index], &name[index..]),
        _ => (name, ""),
    }
}

/// 重命名输入框应当预选的**字节**区间：只选主干，不选扩展名。
///
/// 与资源管理器一致：改名时扩展名不被选中，因此直接敲新名字不会把 `.txt` 弄丢。
/// 返回 UTF-8 字节偏移，正是 `InputState::set_selected_range` 要的单位
/// （它会把落在字符中间的偏移夹到字符边界，因此中文名也安全）。
pub fn rename_selection_range(name: &str) -> Range<usize> {
    let (stem, _) = split_extension(name);
    0..stem.len()
}

/// 在 `taken` 认为「已被占用」时不断往后找，返回第一个可用名称。
///
/// 命名规则与资源管理器一致：序号插在**主干与扩展名之间**，从 2 起。
/// 插在中间而不是末尾，是为了让 `报告 (2).docx` 仍然被当成 `.docx` 文件——
/// `报告.docx (2)` 会让后续的类型判断与关联全部失配。
///
/// `taken` 由调用方提供（真实实现查文件系统，单测里查一个集合），
/// 因此这段逻辑可以完全不碰 IO 地测。
pub fn unique_name(desired: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(desired) {
        return desired.to_string();
    }

    let (stem, ext) = split_extension(desired);
    // 去掉自带序号再编号：否则反复粘贴「报告 (2).docx」会得到
    // 「报告 (2) (2).docx」，而资源管理器给的是「报告 (3).docx」。
    let base = strip_index_suffix(stem);

    for index in 2..=MAX_SUFFIX {
        let candidate = format!("{base} ({index}){ext}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    desired.to_string()
}

/// 去掉主干末尾的 ` (数字)` 序号（仅当它是纯数字且无前导零）。
///
/// 保留 `(02)` 这类带前导零的名字：那看起来像用户自己起的，
/// 而不是我们生成的序号。
fn strip_index_suffix(stem: &str) -> &str {
    let Some(open) = stem.rfind(" (") else {
        return stem;
    };
    let Some(digits) = stem[open + 2..].strip_suffix(')') else {
        return stem;
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return stem;
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return stem;
    }
    &stem[..open]
}

/// 批量重命名的参数。
///
/// 刻意只有「前缀 + 起始序号 + 是否保留扩展名」三项：
/// 更复杂的模板（正则、占位符）会把「批量改名」变成一门小语言，
/// 而用户真正需要的是「把这些照片改成 旅行_1、旅行_2……」。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchSpec {
    /// 序号前面的部分，可以为空（此时名称就是纯序号）。
    pub prefix: String,
    /// 第一个序号。
    pub start: u32,
    /// 是否保留原扩展名。
    ///
    /// 默认开启：把 `报告.docx` 改成 `报告_1` 会让它失去文件类型，
    /// 而用户想改的几乎总是主干。
    pub keep_extension: bool,
}

impl Default for BatchSpec {
    fn default() -> Self {
        Self {
            prefix: String::from("文件_"),
            start: 1,
            keep_extension: true,
        }
    }
}

/// 批量重命名无法生成名称的原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchError {
    /// 没有选中任何条目。
    EmptySelection,
    /// 生成的某个名称不合法（前缀里带了 `?` 之类）。
    Invalid(NameError),
    /// 生成了重复的名称。
    ///
    /// 正常情况下序号天然唯一；这条只在序号溢出（`start` 极大）时才可能触发，
    /// 但一旦发生就是「两个条目要改成同一个名字」，必须在提交前拦住。
    Duplicate(String),
}

impl BatchError {
    /// 面向用户的一句话说明。
    pub fn message(&self) -> String {
        match self {
            Self::EmptySelection => String::from("先选中要改名的项目"),
            Self::Invalid(error) => error.message(),
            Self::Duplicate(name) => format!("会生成重复的名称「{name}」"),
        }
    }
}

/// 按 [`BatchSpec`] 生成新的名称序列（**不碰文件系统**）。
///
/// 输入与输出一一对应、顺序保持：调用方拿 `(原路径, 新名字)` 直接执行即可。
/// 扩展名从原名称里取（见 [`split_extension`]），因此 `archive.tar.gz` 会变成
/// `前缀1.gz` —— 与「重命名只改主干」的既有约定一致。
pub fn batch_names(names: &[&str], spec: &BatchSpec) -> Result<Vec<String>, BatchError> {
    if names.is_empty() {
        return Err(BatchError::EmptySelection);
    }

    // 去首尾空白只做一次：前缀末尾的空格会产生「文件_ 1」这种名字。
    let prefix = spec.prefix.trim_end();
    let mut seen: HashSet<String> = HashSet::with_capacity(names.len());
    let mut result = Vec::with_capacity(names.len());

    for (offset, name) in names.iter().enumerate() {
        let (_, extension) = split_extension(name);
        let index = spec.start.saturating_add(offset as u32);
        let stem = format!("{prefix}{index}");
        let candidate = if spec.keep_extension {
            format!("{stem}{extension}")
        } else {
            stem
        };

        let valid = validate(&candidate).map_err(BatchError::Invalid)?;
        // Windows 的文件名不区分大小写，因此查重也按不区分大小写做。
        if !seen.insert(valid.to_lowercase()) {
            return Err(BatchError::Duplicate(valid.to_string()));
        }
        result.push(valid.to_string());
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_trims_and_accepts_a_normal_name() {
        assert_eq!(validate("  报告.docx ").unwrap(), "报告.docx");
        assert_eq!(validate("新建文件夹").unwrap(), "新建文件夹");
    }

    #[test]
    fn validate_rejects_empty_and_whitespace_only() {
        assert_eq!(validate(""), Err(NameError::Empty));
        assert_eq!(validate("   "), Err(NameError::Empty));
        assert_eq!(validate("\t\n"), Err(NameError::Empty));
    }

    #[test]
    fn validate_rejects_each_forbidden_character() {
        for bad in ['<', '>', ':', '"', '/', '\\', '|', '?', '*'] {
            let name = format!("a{bad}b");
            assert_eq!(
                validate(&name),
                Err(NameError::Forbidden(bad)),
                "{bad} 必须被拒绝"
            );
        }
    }

    #[test]
    fn validate_rejects_control_characters_before_forbidden_ones() {
        // 控制字符的展示形式是空白或方框，因此必须优先报出，
        // 否则用户看到的是「不能包含字符『』」。
        assert_eq!(validate("a\u{1}b"), Err(NameError::Control));
        assert_eq!(validate("a\u{7f}b"), Err(NameError::Control));
    }

    #[test]
    fn validate_rejects_trailing_dot() {
        assert_eq!(validate("a.txt."), Err(NameError::TrailingDot));
        assert_eq!(validate("."), Err(NameError::TrailingDot));
        assert_eq!(validate(".."), Err(NameError::TrailingDot));
    }

    #[test]
    fn validate_tolerates_a_trailing_space_by_trimming_it() {
        // Windows 会把尾随空格静默吃掉。与其让用户拿到一个和输入不同的名字，
        // 不如按 trim 后的结果建——这正是设计稿 `e.value.trim()` 的行为。
        assert_eq!(validate("报告 ").unwrap(), "报告");
    }

    #[test]
    fn validate_rejects_reserved_device_names_case_insensitively() {
        for bad in ["CON", "nul", "Com1", "LPT9", "aux.txt", "NUL.log"] {
            assert_eq!(
                validate(bad),
                Err(NameError::ReservedDevice),
                "{bad} 是保留设备名"
            );
        }
        // 前缀相同但不是设备名的必须放行。
        for ok in ["CONSOLE", "COM10", "NULL", "AUX2"] {
            assert!(validate(ok).is_ok(), "{ok} 不该被当成设备名");
        }
    }

    #[test]
    fn validate_counts_length_in_utf16_units() {
        // 128 个 emoji = 256 个 UTF-16 码元 > 255；按 chars 数只有 128，会误判通过。
        let emoji = "😀".repeat(128);
        assert_eq!(validate(&emoji), Err(NameError::TooLong));

        // 255 个 ASCII 字符正好在上限。
        let ascii = "a".repeat(MAX_NAME_UNITS);
        assert!(validate(&ascii).is_ok());

        // 127 个 emoji（254 码元）应当通过。
        assert!(validate(&"😀".repeat(127)).is_ok());
    }

    #[test]
    fn split_extension_matches_explorer() {
        assert_eq!(split_extension("报告.docx"), ("报告", ".docx"));
        assert_eq!(split_extension("archive.tar.gz"), ("archive.tar", ".gz"));
        assert_eq!(split_extension("noext"), ("noext", ""));
        // 开头的点不是分隔符，否则 `.gitignore` 会被当成「没有主干的文件」。
        assert_eq!(split_extension(".gitignore"), (".gitignore", ""));
        // 点后面为空：不是扩展名，否则主干会变成空串。
        assert_eq!(split_extension("a."), ("a.", ""));
    }

    #[test]
    fn rename_selection_covers_the_stem_only() {
        assert_eq!(rename_selection_range("报告.docx"), 0..6, "「报告」是 6 字节");
        assert_eq!(rename_selection_range(".gitignore"), 0..10);
        assert_eq!(rename_selection_range("a.txt."), 0..6, "没有扩展名时全选");
    }

    #[test]
    fn unique_name_returns_the_desired_name_when_free() {
        let taken: HashSet<&str> = HashSet::new();
        assert_eq!(
            unique_name("新建文件夹", |name| taken.contains(name)),
            "新建文件夹"
        );
    }

    #[test]
    fn unique_name_appends_a_number_from_two() {
        let taken: HashSet<&str> = ["新建文件夹"].into_iter().collect();
        assert_eq!(
            unique_name("新建文件夹", |name| taken.contains(name)),
            "新建文件夹 (2)"
        );

        let taken: HashSet<&str> = ["新建文件夹", "新建文件夹 (2)"].into_iter().collect();
        assert_eq!(
            unique_name("新建文件夹", |name| taken.contains(name)),
            "新建文件夹 (3)"
        );
    }

    #[test]
    fn unique_name_inserts_the_number_before_the_extension() {
        // 序号必须插在主干与扩展名之间：`报告.docx (2)` 会让类型判断失配。
        let taken: HashSet<&str> = ["报告.docx"].into_iter().collect();
        assert_eq!(
            unique_name("报告.docx", |name| taken.contains(name)),
            "报告 (2).docx"
        );
    }

    #[test]
    fn unique_name_strips_a_builtin_number_before_numbering() {
        // 反复粘贴「报告 (2).docx」应当得到「报告 (3).docx」，
        // 而不是「报告 (2) (2).docx」。
        let taken: HashSet<&str> = ["报告 (2).docx"].into_iter().collect();
        assert_eq!(
            unique_name("报告 (2).docx", |name| taken.contains(name)),
            "报告 (3).docx"
        );
    }

    #[test]
    fn unique_name_keeps_zero_padded_suffixes() {
        // `(02)` 看起来像用户自己起的名，不能当作我们生成的序号抹掉。
        let taken: HashSet<&str> = ["会议 (02)"].into_iter().collect();
        assert_eq!(
            unique_name("会议 (02)", |name| taken.contains(name)),
            "会议 (02) (2)"
        );
    }

    #[test]
    fn batch_names_numbers_from_the_requested_start() {
        let spec = BatchSpec {
            prefix: String::from("旅行_"),
            start: 3,
            keep_extension: true,
        };
        let names = batch_names(&["a.jpg", "b.jpg"], &spec).unwrap();
        assert_eq!(names, vec!["旅行_3.jpg", "旅行_4.jpg"]);
    }

    #[test]
    fn batch_names_keep_the_original_extension_per_file() {
        // 每个条目各自保留自己的扩展名：一次批量操作里可能混着 .jpg 与 .png。
        let spec = BatchSpec::default();
        let names = batch_names(&["a.jpg", "b.png", "无扩展名", ".gitignore"], &spec).unwrap();
        assert_eq!(
            names,
            vec!["文件_1.jpg", "文件_2.png", "文件_3", "文件_4"]
        );
        // `.gitignore` 没有扩展名（开头的点不算），因此序号直接顶上去。
        assert_eq!(split_extension(".gitignore").1, "");
    }

    #[test]
    fn batch_names_can_drop_the_extension() {
        let spec = BatchSpec {
            prefix: String::from("x"),
            start: 1,
            keep_extension: false,
        };
        assert_eq!(batch_names(&["a.txt"], &spec).unwrap(), vec!["x1"]);
    }

    #[test]
    fn batch_names_reject_an_invalid_prefix() {
        let spec = BatchSpec {
            prefix: String::from("a?b"),
            start: 1,
            keep_extension: true,
        };
        assert_eq!(
            batch_names(&["a.txt"], &spec),
            Err(BatchError::Invalid(NameError::Forbidden('?')))
        );
    }

    #[test]
    fn batch_names_reject_duplicates_caused_by_index_overflow() {
        // `start` 极大时 `saturating_add` 会把多个序号压成同一个数字，
        // 那会生成重复名称——必须在提交前拦住，而不是让第二条 rename 失败。
        let spec = BatchSpec {
            prefix: String::new(),
            start: u32::MAX,
            keep_extension: false,
        };
        assert!(matches!(
            batch_names(&["a.txt", "b.txt"], &spec),
            Err(BatchError::Duplicate(_))
        ));
    }

    #[test]
    fn batch_names_need_a_selection() {
        assert_eq!(
            batch_names(&[], &BatchSpec::default()),
            Err(BatchError::EmptySelection)
        );
    }

    #[test]
    fn unique_name_gives_up_instead_of_looping_forever() {
        // 极端情况：所有候选都被占用。此时返回原名，由系统去报「已存在」，
        // 而不是在这里死循环。
        assert_eq!(unique_name("x", |_| true), "x");
    }
}
