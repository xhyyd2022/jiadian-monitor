//! 外设协议 JSON 的反序列化结构。
//!
//! 字段含义见 `peripherals/README.md`。原则：**未知字段一律忽略**，缺省字段给安全默认值，
//! 这样用户自己加注释性字段也不会让整份配置加载失败。

use serde::de::{self, Deserializer, Visitor};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

/// 解析十六进制字面量：`"1532"`、`"0x1532"`、`"0X1532"` 都可以。
pub fn hex_u32(s: &str) -> Result<u32, String> {
    let t = s.trim();
    let t = t
        .strip_prefix("0x")
        .or_else(|| t.strip_prefix("0X"))
        .unwrap_or(t);
    if t.is_empty() {
        return Err(format!("十六进制数为空：{s:?}"));
    }
    u32::from_str_radix(t, 16).map_err(|e| format!("十六进制数 {s:?} 解析失败：{e}"))
}

/// 单个字节。接受 `"1f"` / `"0x1f"` / `31`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HexByte(pub u8);

impl<'de> Deserialize<'de> for HexByte {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<HexByte, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = HexByte;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("十六进制字符串或字节整数")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<HexByte, E> {
                let n = hex_u32(v).map_err(E::custom)?;
                if n > 0xFF {
                    return Err(E::custom(format!("字节 {v:?} 超过 0xFF")));
                }
                Ok(HexByte(n as u8))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<HexByte, E> {
                if v > 0xFF {
                    return Err(E::custom(format!("字节 {v} 超过 0xFF")));
                }
                Ok(HexByte(v as u8))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<HexByte, E> {
                if !(0..=0xFF).contains(&v) {
                    return Err(E::custom(format!("字节 {v} 超过 0xFF")));
                }
                Ok(HexByte(v as u8))
            }
        }
        d.deserialize_any(V)
    }
}

/// 16 位。接受十六进制字符串或数字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HexU16(pub u16);

impl<'de> Deserialize<'de> for HexU16 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<HexU16, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = HexU16;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("十六进制字符串或 16 位整数")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<HexU16, E> {
                let n = hex_u32(v).map_err(E::custom)?;
                if n > 0xFFFF {
                    return Err(E::custom(format!("{v:?} 超过 0xFFFF")));
                }
                Ok(HexU16(n as u16))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<HexU16, E> {
                if v > 0xFFFF {
                    return Err(E::custom(format!("{v} 超过 0xFFFF")));
                }
                Ok(HexU16(v as u16))
            }
        }
        d.deserialize_any(V)
    }
}

/// `vid` / `pid`：`"*"` 表示通配。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VidPid {
    Exact(u16),
    Any,
}

#[allow(dead_code)]
impl VidPid {
    pub fn matches(&self, v: u16) -> bool {
        match self {
            VidPid::Exact(n) => *n == v,
            VidPid::Any => true,
        }
    }
    /// 排序用：精确值优先于通配。
    pub fn rank(&self) -> u8 {
        match self {
            VidPid::Exact(_) => 0,
            VidPid::Any => 1,
        }
    }
    pub fn label(&self) -> String {
        match self {
            VidPid::Exact(n) => format!("{n:04X}"),
            VidPid::Any => "*".into(),
        }
    }
}

impl<'de> Deserialize<'de> for VidPid {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<VidPid, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = VidPid;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("十六进制字符串，或 \"*\" 表示全部")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<VidPid, E> {
                if v.trim() == "*" {
                    return Ok(VidPid::Any);
                }
                let n = hex_u32(v).map_err(E::custom)?;
                if n > 0xFFFF {
                    return Err(E::custom(format!("{v:?} 超过 0xFFFF")));
                }
                Ok(VidPid::Exact(n as u16))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<VidPid, E> {
                Ok(VidPid::Exact(v as u16))
            }
        }
        d.deserialize_any(V)
    }
}

// ---------------------------------------------------------------- index.json

// 这些结构只承载 JSON 内容，部分字段是给人看的说明 / 供后续使用，不强求都读到。
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct Index {
    #[serde(default = "one")]
    pub schema: u32,
    #[serde(default)]
    pub poll: Poll,
    #[serde(default)]
    pub brands: BTreeMap<String, BrandRef>,
    #[serde(default)]
    pub devices: Vec<DeviceEntry>,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Poll {
    #[serde(default = "ms3000")]
    pub interval_ms: u64,
}

fn ms3000() -> u64 {
    3000
}

impl Default for Poll {
    fn default() -> Self {
        Poll { interval_ms: 3000 }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct BrandRef {
    #[serde(default)]
    pub name: String,
    pub file: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceEntry {
    pub vid: VidPid,
    pub pid: VidPid,
    pub brand: String,
    #[serde(default)]
    pub protocol: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

impl DeviceEntry {
    /// 匹配优先级：VID、PID 都精确的排最前。
    pub fn rank(&self) -> (u8, u8) {
        (self.vid.rank(), self.pid.rank())
    }
    pub fn accepts(&self, vid: u16, pid: u16) -> bool {
        self.vid.matches(vid) && self.pid.matches(pid)
    }
}

// ------------------------------------------------------- brands/<品牌>.json

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct BrandFile {
    #[serde(default)]
    pub brand: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub protocols: BTreeMap<String, Protocol>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Feature,
    Output,
    Input,
}

#[allow(dead_code)]
impl Transport {
    pub fn label(&self) -> &'static str {
        match self {
            Transport::Feature => "feature",
            Transport::Output => "output",
            Transport::Input => "input",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct Protocol {
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default, rename = "match")]
    pub match_spec: Option<MatchSpec>,
    pub transport: Transport,
    #[serde(default)]
    pub pre: Vec<PreStep>,
    #[serde(default)]
    pub tx: Option<TxSpec>,
    #[serde(default)]
    pub rx: Option<RxSpec>,
    pub battery: Battery,
    #[serde(default)]
    pub fields: Vec<FieldSpec>,
    #[serde(default)]
    pub sample: Option<Sample>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchSpec {
    #[serde(default)]
    pub usage_page: Option<HexU16>,
    #[serde(default)]
    pub usage: Option<HexU16>,
    #[serde(default)]
    pub input_length: Option<u16>,
    #[serde(default)]
    pub output_length: Option<u16>,
    #[serde(default)]
    pub feature_length: Option<u16>,
    #[serde(default)]
    pub product_like: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum TxSpec {
    One(TxFrame),
    Many(Vec<TxFrame>),
}

impl TxSpec {
    pub fn frames(&self) -> Vec<TxFrame> {
        match self {
            TxSpec::One(f) => vec![f.clone()],
            TxSpec::Many(v) => v.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TxFrame {
    pub length: usize,
    #[serde(default)]
    pub report_id: Option<HexByte>,
    #[serde(default)]
    pub pad: Option<HexByte>,
    #[serde(default)]
    pub bytes: BTreeMap<String, HexByte>,
    #[serde(default)]
    pub checksum: Option<Checksum>,
    #[serde(default)]
    pub delay_ms: Option<u64>,
    /// 只在 JSON 里给人看，程序不读（保留是为了让协议文件自解释）。
    #[allow(dead_code)]
    #[serde(default)]
    pub desc: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct Checksum {
    pub put: usize,
    pub expr: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RxSpec {
    #[serde(default)]
    pub length: Option<usize>,
    #[serde(default, rename = "match")]
    pub match_rules: Vec<RxRule>,
    #[serde(default)]
    pub skip: Vec<RxRule>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub attempts: Option<u32>,
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// 只在 JSON 里给人看，程序不读。
    #[allow(dead_code)]
    #[serde(default)]
    pub note: Option<String>,
}

impl RxSpec {
    pub fn timeout(&self) -> u64 {
        self.timeout_ms.unwrap_or(300)
    }
    pub fn attempts(&self) -> u32 {
        self.attempts.unwrap_or(3).max(1)
    }
    pub fn interval(&self) -> u64 {
        self.interval_ms.unwrap_or(40)
    }
    pub fn accept(&self, buf: &[u8]) -> bool {
        self.match_rules.iter().all(|r| r.hit(buf))
    }
    pub fn should_skip(&self, buf: &[u8]) -> bool {
        self.skip.iter().any(|r| r.hit(buf))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RxRule {
    pub offset: usize,
    pub equals: HexByte,
}

impl RxRule {
    pub fn hit(&self, buf: &[u8]) -> bool {
        buf.get(self.offset) == Some(&self.equals.0)
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct PreStep {
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub tx: Option<TxSpec>,
    #[serde(default)]
    pub rx: Option<RxSpec>,
    #[serde(default)]
    pub take: Option<TakeSpec>,
    #[serde(default)]
    pub put: Option<PutSpec>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct TakeSpec {
    pub offset: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct PutSpec {
    pub frame: usize,
    pub offset: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct Battery {
    pub expr: String,
    #[serde(default)]
    pub round: Option<bool>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub valid: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct AsciiSpec {
    pub offset: usize,
    pub length: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct FieldSpec {
    pub name: String,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub expr: Option<String>,
    #[serde(default)]
    pub ascii: Option<AsciiSpec>,
    #[serde(default)]
    pub round: Option<bool>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub valid: Option<String>,
    #[serde(default)]
    pub tx: Option<TxSpec>,
    #[serde(default)]
    pub rx: Option<RxSpec>,
    #[serde(default)]
    pub note: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct Sample {
    pub rx: String,
    #[serde(default)]
    pub expect: SampleExpect,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SampleExpect {
    #[serde(default)]
    pub battery: Option<f64>,
    #[serde(default)]
    pub charging: Option<bool>,
    #[serde(default)]
    pub model: Option<String>,
}

/// 把 `"a1 00 02 ..."` / `"a10002"` / `"0xA1,0x00"` 都解析成字节。
///
/// 先按空白 / `,` / `-` / `_` 切成 token，每个 token 各去掉自己的 `0x` 前缀再拼接，
/// 这样 `"0xA1,0x00"` 不会被拼成 `"0xA10x00"`。
pub fn parse_hex_bytes(s: &str) -> Result<Vec<u8>, String> {
    let mut digits = String::with_capacity(s.len());
    for tok in s.split(|c: char| c.is_whitespace() || c == ',' || c == '-' || c == '_') {
        if tok.is_empty() {
            continue;
        }
        let t = tok
            .strip_prefix("0x")
            .or_else(|| tok.strip_prefix("0X"))
            .unwrap_or(tok);
        digits.push_str(t);
    }
    let t: &str = &digits;
    if t.len() % 2 != 0 {
        return Err(format!("十六进制串长度为奇数（{} 个字符）", t.len()));
    }
    let mut out = Vec::with_capacity(t.len() / 2);
    let b = t.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let hi = (b[i] as char)
            .to_digit(16)
            .ok_or_else(|| format!("非法十六进制字符 {:?}", b[i] as char))?;
        let lo = (b[i + 1] as char)
            .to_digit(16)
            .ok_or_else(|| format!("非法十六进制字符 {:?}", b[i + 1] as char))?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_literals() {
        assert_eq!(hex_u32("1532").unwrap(), 0x1532);
        assert_eq!(hex_u32("0x1532").unwrap(), 0x1532);
        assert_eq!(hex_u32("0Xab").unwrap(), 0xAB);
        assert!(hex_u32("").is_err());
        assert!(hex_u32("zz").is_err());
    }

    #[test]
    fn hex_bytes() {
        assert_eq!(parse_hex_bytes("a1 00 02").unwrap(), vec![0xA1, 0x00, 0x02]);
        assert_eq!(parse_hex_bytes("a10002").unwrap(), vec![0xA1, 0x00, 0x02]);
        assert_eq!(parse_hex_bytes("0xA1,0x00").unwrap(), vec![0xA1, 0x00]);
        assert_eq!(parse_hex_bytes("0xA1 0x00 0x02").unwrap(), vec![0xA1, 0x00, 0x02]);
        assert!(parse_hex_bytes("a1 0").is_err());
        assert!(parse_hex_bytes("zz").is_err());
    }

    /// 数据目录：`src-tauri/src/peripherals/../../../peripherals` → `app/peripherals`。
    fn data_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("peripherals")
    }

    fn load_index() -> Index {
        let p = data_dir().join("index.json");
        let src = std::fs::read_to_string(&p)
            .unwrap_or_else(|e| panic!("读不到 {}：{e}", p.display()));
        serde_json::from_str(&src).unwrap_or_else(|e| panic!("{} 解析失败：{e}", p.display()))
    }

    #[test]
    fn index_parses() {
        let idx = load_index();
        assert_eq!(idx.schema, 1);
        assert!(idx.brands.contains_key("razer"));
        assert!(!idx.devices.is_empty());
        for d in &idx.devices {
            assert!(idx.brands.contains_key(&d.brand), "未知品牌 {}", d.brand);
        }
    }

    #[test]
    fn brands_parse() {
        let idx = load_index();
        for (key, r) in &idx.brands {
            let p = data_dir().join(&r.file);
            let src = std::fs::read_to_string(&p)
                .unwrap_or_else(|e| panic!("读不到 {}：{e}", p.display()));
            let bf: BrandFile = serde_json::from_str(&src)
                .unwrap_or_else(|e| panic!("{} 解析失败：{e}", p.display()));
            assert_eq!(&bf.brand, key, "{} 的 brand 字段与 index key 不符", r.file);
            assert!(!bf.protocols.is_empty(), "{} 没有协议档位", r.file);
            for (pn, pr) in &bf.protocols {
                assert!(
                    !pr.battery.expr.trim().is_empty(),
                    "{}/{pn} 的 battery.expr 为空",
                    r.file
                );
            }
        }
    }

    #[test]
    fn index_protocol_refs_exist() {
        let idx = load_index();
        for d in &idx.devices {
            let r = &idx.brands[&d.brand];
            let p = data_dir().join(&r.file);
            let src = std::fs::read_to_string(&p).unwrap();
            let bf: BrandFile = serde_json::from_str(&src).unwrap();
            let name = d
                .protocol
                .clone()
                .or_else(|| bf.default.clone())
                .unwrap_or_else(|| panic!("{} 没有 default 档位", r.file));
            assert!(
                bf.protocols.contains_key(&name),
                "index 引用了 {}/{} 里不存在的档位 {name}",
                r.file,
                d.brand
            );
        }
    }

    #[test]
    fn tx_frame_camel_case() {
        let f: TxFrame = serde_json::from_str(
            r#"{"length":20,"reportId":"11","bytes":{"1":"ff"},"delayMs":5}"#,
        )
        .unwrap();
        assert_eq!(f.length, 20);
        assert_eq!(f.report_id.unwrap().0, 0x11);
        assert_eq!(f.bytes["1"].0, 0xFF);
        assert_eq!(f.delay_ms, Some(5));
    }
}
