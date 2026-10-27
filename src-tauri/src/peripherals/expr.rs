//! 协议 JSON 里的安全表达式求值。
//!
//! 只支持算术、位运算、比较和一小批函数，**不会执行任意代码**。
//! 语义与 `peripherals/_check.py` 的参照实现保持一致，两边必须同步改。

const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Byte(usize),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let b: Vec<char> = src.chars().collect();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '(' {
            out.push(Tok::LParen);
            i += 1;
            continue;
        }
        if c == ')' {
            out.push(Tok::RParen);
            i += 1;
            continue;
        }
        if c == ',' {
            out.push(Tok::Comma);
            i += 1;
            continue;
        }
        // 双字符运算符
        if i + 1 < b.len() {
            let two: String = [b[i], b[i + 1]].iter().collect();
            let op = match two.as_str() {
                "==" => Some("=="),
                "!=" => Some("!="),
                "<=" => Some("<="),
                ">=" => Some(">="),
                "<<" => Some("<<"),
                ">>" => Some(">>"),
                "&&" => Some("&&"),
                "||" => Some("||"),
                _ => None,
            };
            if let Some(op) = op {
                out.push(Tok::Op(op));
                i += 2;
                continue;
            }
        }
        if c.is_ascii_digit() {
            let start = i;
            if c == '0' && i + 1 < b.len() && (b[i + 1] == 'x' || b[i + 1] == 'X') {
                i += 2;
                let hs = i;
                while i < b.len() && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
                if i == hs {
                    return Err("0x 后面没有数字".into());
                }
                let s: String = b[hs..i].iter().collect();
                let v = i64::from_str_radix(&s, 16)
                    .map_err(|e| format!("十六进制数 {s} 解析失败：{e}"))?;
                out.push(Tok::Num(v as f64));
                continue;
            }
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i < b.len() && b[i] == '.' {
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let s: String = b[start..i].iter().collect();
            let v: f64 = s.parse().map_err(|e| format!("数字 {s} 解析失败：{e}"))?;
            out.push(Tok::Num(v));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_') {
                i += 1;
            }
            let name: String = b[start..i].iter().collect();
            // b[6] 形式
            if name == "b" && i < b.len() && b[i] == '[' {
                let save = i;
                i += 1;
                let ds = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                if i == ds || i >= b.len() || b[i] != ']' {
                    i = save;
                    out.push(Tok::Ident(name));
                    continue;
                }
                let idx: usize = b[ds..i]
                    .iter()
                    .collect::<String>()
                    .parse()
                    .map_err(|e| format!("b[] 下标解析失败：{e}"))?;
                i += 1; // 吃掉 ]
                out.push(Tok::Byte(idx));
                continue;
            }
            out.push(Tok::Ident(name));
            continue;
        }
        let op = match c {
            '+' => "+",
            '-' => "-",
            '*' => "*",
            '/' => "/",
            '%' => "%",
            '&' => "&",
            '|' => "|",
            '^' => "^",
            '~' => "~",
            '!' => "!",
            '<' => "<",
            '>' => ">",
            _ => return Err(format!("表达式里出现无法识别的字符 {c:?}")),
        };
        out.push(Tok::Op(op));
        i += 1;
    }
    Ok(out)
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    buf: &'a [u8],
    depth: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat_op(&mut self, target: &str) -> bool {
        if let Some(Tok::Op(o)) = self.peek() {
            if *o == target {
                self.pos += 1;
                return true;
            }
        }
        false
    }

    fn byte_at(&self, i: usize) -> Result<f64, String> {
        self.buf
            .get(i)
            .map(|v| *v as f64)
            .ok_or_else(|| format!("偏移 {i} 越界（缓冲区长 {}）", self.buf.len()))
    }

    fn expr(&mut self) -> Result<f64, String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("表达式嵌套太深".into());
        }
        let r = self.or();
        self.depth -= 1;
        r
    }

    fn or(&mut self) -> Result<f64, String> {
        let mut v = self.and()?;
        while self.eat_op("||") {
            let r = self.and()?;
            v = if v != 0.0 || r != 0.0 { 1.0 } else { 0.0 };
        }
        Ok(v)
    }

    fn and(&mut self) -> Result<f64, String> {
        let mut v = self.bor()?;
        while self.eat_op("&&") {
            let r = self.bor()?;
            v = if v != 0.0 && r != 0.0 { 1.0 } else { 0.0 };
        }
        Ok(v)
    }

    fn bor(&mut self) -> Result<f64, String> {
        let mut v = self.bxor()?;
        while self.eat_op("|") {
            let r = self.bxor()?;
            v = ((v as i64) | (r as i64)) as f64;
        }
        Ok(v)
    }

    fn bxor(&mut self) -> Result<f64, String> {
        let mut v = self.band()?;
        while self.eat_op("^") {
            let r = self.band()?;
            v = ((v as i64) ^ (r as i64)) as f64;
        }
        Ok(v)
    }

    fn band(&mut self) -> Result<f64, String> {
        let mut v = self.cmp()?;
        while self.eat_op("&") {
            let r = self.cmp()?;
            v = ((v as i64) & (r as i64)) as f64;
        }
        Ok(v)
    }

    fn cmp(&mut self) -> Result<f64, String> {
        let mut v = self.add()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) if ["==", "!=", "<", "<=", ">", ">="].contains(o) => *o,
                _ => break,
            };
            self.pos += 1;
            let r = self.add()?;
            v = match op {
                "==" => (v == r) as i64 as f64,
                "!=" => (v != r) as i64 as f64,
                "<" => (v < r) as i64 as f64,
                "<=" => (v <= r) as i64 as f64,
                ">" => (v > r) as i64 as f64,
                _ => (v >= r) as i64 as f64,
            };
        }
        Ok(v)
    }

    fn add(&mut self) -> Result<f64, String> {
        let mut v = self.mul()?;
        loop {
            if self.eat_op("+") {
                v += self.mul()?;
            } else if self.eat_op("-") {
                v -= self.mul()?;
            } else {
                break;
            }
        }
        Ok(v)
    }

    fn mul(&mut self) -> Result<f64, String> {
        let mut v = self.unary()?;
        loop {
            if self.eat_op("*") {
                v *= self.unary()?;
            } else if self.eat_op("/") {
                let r = self.unary()?;
                v = if r == 0.0 { 0.0 } else { v / r };
            } else if self.eat_op("%") {
                let r = self.unary()?;
                v = if r == 0.0 { 0.0 } else { v % r };
            } else {
                break;
            }
        }
        Ok(v)
    }

    fn unary(&mut self) -> Result<f64, String> {
        if self.eat_op("-") {
            return Ok(-self.unary()?);
        }
        if self.eat_op("+") {
            return self.unary();
        }
        if self.eat_op("!") {
            let v = self.unary()?;
            return Ok(if v == 0.0 { 1.0 } else { 0.0 });
        }
        if self.eat_op("~") {
            let v = self.unary()?;
            return Ok((!(v as i64)) as f64);
        }
        self.shift()
    }

    fn shift(&mut self) -> Result<f64, String> {
        let mut v = self.atom()?;
        loop {
            if self.eat_op("<<") {
                let r = self.atom()? as i64;
                v = (((v as i64) << r.clamp(0, 63)) as f64).max(f64::MIN);
            } else if self.eat_op(">>") {
                let r = self.atom()? as i64;
                v = (((v as i64) >> r.clamp(0, 63)) as f64).max(f64::MIN);
            } else {
                break;
            }
        }
        Ok(v)
    }

    fn atom(&mut self) -> Result<f64, String> {
        match self.next() {
            Some(Tok::Num(v)) => Ok(v),
            Some(Tok::Byte(i)) => self.byte_at(i),
            Some(Tok::LParen) => {
                let v = self.expr()?;
                if !matches!(self.next(), Some(Tok::RParen)) {
                    return Err("缺少右括号".into());
                }
                Ok(v)
            }
            Some(Tok::Ident(name)) => {
                let mut args = Vec::new();
                if matches!(self.peek(), Some(Tok::LParen)) {
                    self.pos += 1;
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        args.push(self.expr()?);
                        while matches!(self.peek(), Some(Tok::Comma)) {
                            self.pos += 1;
                            args.push(self.expr()?);
                        }
                    }
                    if !matches!(self.next(), Some(Tok::RParen)) {
                        return Err("函数调用缺少右括号".into());
                    }
                }
                self.call(&name, &args)
            }
            other => Err(format!("表达式里出现意外的记号 {other:?}")),
        }
    }

    fn call(&mut self, name: &str, a: &[f64]) -> Result<f64, String> {
        let need = |n: usize| -> Result<(), String> {
            if a.len() < n {
                Err(format!("函数 {name} 需要 {n} 个参数，实际给了 {}", a.len()))
            } else {
                Ok(())
            }
        };
        match name {
            "u16le" => {
                need(1)?;
                let o = a[0] as usize;
                Ok(self.byte_at(o)? + self.byte_at(o + 1)? * 256.0)
            }
            "u16be" => {
                need(1)?;
                let o = a[0] as usize;
                Ok(self.byte_at(o)? * 256.0 + self.byte_at(o + 1)?)
            }
            "s16le" => {
                need(1)?;
                let o = a[0] as usize;
                let v = self.byte_at(o)? as i64 + (self.byte_at(o + 1)? as i64) * 256;
                Ok(if v & 0x8000 != 0 { (v - 0x10000) as f64 } else { v as f64 })
            }
            "s8" => {
                need(1)?;
                let v = self.byte_at(a[0] as usize)? as i64;
                Ok(if v & 0x80 != 0 { (v - 0x100) as f64 } else { v as f64 })
            }
            "min" => {
                need(1)?;
                Ok(a.iter().cloned().fold(f64::INFINITY, f64::min))
            }
            "max" => {
                need(1)?;
                Ok(a.iter().cloned().fold(f64::NEG_INFINITY, f64::max))
            }
            "abs" => {
                need(1)?;
                Ok(a[0].abs())
            }
            "round" => {
                need(1)?;
                Ok(a[0].round())
            }
            "clamp" => {
                need(3)?;
                Ok(a[0].max(a[1]).min(a[2]))
            }
            "if" => {
                need(3)?;
                Ok(if a[0] != 0.0 { a[1] } else { a[2] })
            }
            "sum" | "xor" => {
                need(2)?;
                let from = a[0] as usize;
                let to = (a[1] as usize).min(self.buf.len().saturating_sub(1));
                if from > to {
                    return Ok(0.0);
                }
                let mut acc: i64 = if name == "sum" { 0 } else { 0 };
                for i in from..=to {
                    let v = self.byte_at(i)? as i64;
                    if name == "sum" {
                        acc = acc.wrapping_add(v);
                    } else {
                        acc ^= v;
                    }
                }
                Ok(acc as f64)
            }
            "len" => Ok(self.buf.len() as f64),
            other => Err(format!("未知函数 {other}")),
        }
    }
}

/// 对一块缓冲区求值。`src` 为 JSON 里的表达式文本。
pub fn eval(buf: &[u8], src: &str) -> Result<f64, String> {
    let toks = tokenize(src)?;
    if toks.is_empty() {
        return Err("表达式为空".into());
    }
    let mut p = Parser { toks, pos: 0, buf, depth: 0 };
    let v = p.expr()?;
    if p.pos != p.toks.len() {
        return Err(format!("表达式尾巴多余：{:?}", &p.toks[p.pos..]));
    }
    if v.is_nan() || v.is_infinite() {
        return Err("表达式结果不是有限数".into());
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_arith() {
        let b = [0u8, 10, 20, 30];
        assert_eq!(eval(&b, "b[1] + b[2]").unwrap(), 30.0);
        assert_eq!(eval(&b, "b[3] * 100 / 255").unwrap().round(), 12.0);
        assert_eq!(eval(&b, "0x10 + 1").unwrap(), 17.0);
    }

    #[test]
    fn precedence() {
        let b = [0u8, 0x45];
        // 位运算优先级低于比较，高于 &&
        assert_eq!(eval(&b, "(b[1] & 0x0f) <= 5").unwrap(), 1.0);
        // C 系优先级：`==` 比 `&` 紧，所以这一条等价于 `b[1] & (0x0f == 5)` → 0。
        // 想表达「低 4 位等于 5」必须加括号，JSON 里也都加了括号。
        assert_eq!(eval(&b, "b[1] & 0x0f == 5").unwrap(), 0.0);
        assert_eq!(eval(&b, "(b[1] & 0x0f) == 5").unwrap(), 1.0);
        assert_eq!(eval(&b, "b[1] == 5 || b[1] == 0x45").unwrap(), 1.0);
    }

    #[test]
    fn helpers() {
        let b = [0u8, 0x34, 0x12];
        assert_eq!(eval(&b, "u16le(1)").unwrap(), 0x1234 as f64);
        assert_eq!(eval(&b, "u16be(1)").unwrap(), 0x3412 as f64);
        assert_eq!(eval(&b, "sum(0, 2)").unwrap(), (0x34 + 0x12) as f64);
        assert_eq!(eval(&b, "xor(0, 2)").unwrap(), (0x26) as f64);
        assert_eq!(eval(&b, "if(b[1] > 16, 20, 10)").unwrap(), 20.0);
        assert_eq!(eval(&b, "clamp(200, 0, 100)").unwrap(), 100.0);
    }

    #[test]
    fn out_of_range_errors() {
        let b = [1u8, 2];
        assert!(eval(&b, "b[9]").is_err());
        assert!(eval(&b, "b[1] +").is_err());
        assert!(eval(&b, "nope(1)").is_err());
    }
}
