//! kubectl exec query-string parsing (exec protocol split, step 1).

use std::collections::HashMap;

pub fn url_decode(s: &str) -> String {
    let mut bytes: Vec<u8> = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '+' => bytes.push(b' '),
            '%' => {
                let hi = chars.next().and_then(|c| c.to_digit(16)).unwrap_or(0) as u8;
                let lo = chars.next().and_then(|c| c.to_digit(16)).unwrap_or(0) as u8;
                bytes.push(hi * 16 + lo);
            }
            c if c.is_ascii() => bytes.push(c as u8),
            _ => {
                let mut buf = [0u8; 4];
                let encoded = c.encode_utf8(&mut buf);
                bytes.extend_from_slice(encoded.as_bytes());
            }
        }
    }
    String::from_utf8(bytes).unwrap_or_default()
}

pub fn parse_query_params(query: &str) -> HashMap<String, Vec<String>> {
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut parts = pair.splitn(2, '=');
        let key = url_decode(parts.next().unwrap_or(""));
        let value = url_decode(parts.next().unwrap_or(""));
        map.entry(key).or_default().push(value);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_command_query() {
        let m = parse_query_params("command=sh&command=-c&command=echo+hi");
        let cmds = m.get("command").expect("command");
        assert_eq!(cmds, &["sh", "-c", "echo hi"]);
    }
}
