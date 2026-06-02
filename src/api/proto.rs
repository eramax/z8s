/// Minimal k8s protobuf decoder.
///
/// kubectl sends `Content-Type: application/vnd.kubernetes.protobuf` by default.
/// The wire format is: magic `k8s\x00` + protobuf-encoded `runtime.Unknown` wrapper.
/// We decode only the fields we need and convert to `serde_json::Value` so the
/// existing JSON handlers work unchanged.

const MAGIC: &[u8] = b"k8s\x00";

pub fn try_proto_to_json(bytes: &[u8]) -> Option<serde_json::Value> {
    if !bytes.starts_with(MAGIC) {
        return None;
    }
    let mut p = Parser::new(&bytes[4..]);
    let mut api_version = String::new();
    let mut kind = String::new();
    let mut raw: &[u8] = &[];

    while let Some((field, value)) = p.next_field() {
        match (field, value) {
            (1, Value::Bytes(b)) => {
                // TypeMeta
                let mut tp = Parser::new(b);
                while let Some((f2, v2)) = tp.next_field() {
                    match (f2, v2) {
                        (1, Value::Bytes(s)) => api_version = lossy(s),
                        (2, Value::Bytes(s)) => kind = lossy(s),
                        _ => {}
                    }
                }
            }
            (2, Value::Bytes(b)) => raw = b,
            _ => {}
        }
    }

    match kind.as_str() {
        "Namespace" => Some(decode_namespace(raw, &api_version)),
        "Pod" => Some(decode_pod(raw, &api_version)),
        "Deployment" => Some(decode_deployment(raw, &api_version)),
        _ => None,
    }
}

// ── Resource decoders ────────────────────────────────────────────────────────

fn decode_namespace(raw: &[u8], api_version: &str) -> serde_json::Value {
    let meta = parse_object_meta(raw, 1);
    serde_json::json!({
        "apiVersion": api_version,
        "kind": "Namespace",
        "metadata": meta,
    })
}

fn decode_pod(raw: &[u8], api_version: &str) -> serde_json::Value {
    let meta = parse_object_meta(raw, 1);
    let spec = parse_pod_spec(raw, 2);
    serde_json::json!({
        "apiVersion": api_version,
        "kind": "Pod",
        "metadata": meta,
        "spec": spec,
    })
}

fn decode_deployment(raw: &[u8], api_version: &str) -> serde_json::Value {
    let meta = parse_object_meta(raw, 1);
    let spec = parse_deploy_spec(raw, 2);
    serde_json::json!({
        "apiVersion": api_version,
        "kind": "Deployment",
        "metadata": meta,
        "spec": spec,
    })
}

// ── Field parsers ─────────────────────────────────────────────────────────────

fn parse_object_meta(msg: &[u8], field: u32) -> serde_json::Value {
    let Some(inner) = get_bytes_field(msg, field) else {
        return serde_json::json!({});
    };
    let name = get_string_field(inner, 1).unwrap_or_default();
    let generate_name = get_string_field(inner, 2).unwrap_or_default();
    let namespace = get_string_field(inner, 3).unwrap_or_default();
    let uid = get_string_field(inner, 5).unwrap_or_default();

    let mut labels = serde_json::Map::new();
    let mut p = Parser::new(inner);
    while let Some((f, v)) = p.next_field() {
        if f == 11 {
            // map<string,string> encoded as repeated MapEntry messages
            if let Value::Bytes(b) = v {
                if let (Some(k), Some(v)) = (get_string_field(b, 1), get_string_field(b, 2)) {
                    labels.insert(k, serde_json::Value::String(v));
                }
            }
        }
    }

    let mut out = serde_json::json!({ "name": name });
    if !generate_name.is_empty() {
        out["generateName"] = serde_json::json!(generate_name);
    }
    if !namespace.is_empty() {
        out["namespace"] = serde_json::json!(namespace);
    }
    if !uid.is_empty() {
        out["uid"] = serde_json::json!(uid);
    }
    if !labels.is_empty() {
        out["labels"] = serde_json::Value::Object(labels);
    }
    out
}

fn parse_pod_spec(msg: &[u8], field: u32) -> serde_json::Value {
    let Some(inner) = get_bytes_field(msg, field) else {
        return serde_json::json!({});
    };
    let containers = parse_containers(inner, 2);
    let init_containers = parse_containers(inner, 20);
    let mut spec = serde_json::json!({ "containers": containers });
    if !init_containers.is_empty() {
        spec["initContainers"] = serde_json::json!(init_containers);
    }
    if let Some(dns) = get_string_field(inner, 6) {
        spec["dnsPolicy"] = serde_json::json!(dns);
    }
    if let Some(svc) = get_string_field(inner, 8) {
        spec["serviceAccountName"] = serde_json::json!(svc);
    }
    if let Some(restart) = get_string_field(inner, 3) {
        spec["restartPolicy"] = serde_json::json!(restart);
    }
    spec
}

fn parse_deploy_spec(msg: &[u8], field: u32) -> serde_json::Value {
    let Some(inner) = get_bytes_field(msg, field) else {
        return serde_json::json!({});
    };
    let replicas = get_varint_field(inner, 1).unwrap_or(1) as i64;

    // selector (field 2) — LabelSelector
    let selector = get_bytes_field(inner, 2)
        .map(|sel| {
            let mut labels = serde_json::Map::new();
            let mut p = Parser::new(sel);
            while let Some((f, v)) = p.next_field() {
                if f == 1 {
                    if let Value::Bytes(b) = v {
                        if let (Some(k), Some(v)) = (get_string_field(b, 1), get_string_field(b, 2)) {
                            labels.insert(k, serde_json::Value::String(v));
                        }
                    }
                }
            }
            serde_json::json!({ "matchLabels": labels })
        })
        .unwrap_or(serde_json::json!({}));

    // template (field 3) — PodTemplateSpec
    let template = get_bytes_field(inner, 3)
        .map(|tmpl| {
            let tmpl_meta = parse_object_meta(tmpl, 1);
            let tmpl_spec = parse_pod_spec(tmpl, 2);
            serde_json::json!({ "metadata": tmpl_meta, "spec": tmpl_spec })
        })
        .unwrap_or(serde_json::json!({}));

    serde_json::json!({
        "replicas": replicas,
        "selector": selector,
        "template": template,
    })
}

fn parse_containers(msg: &[u8], field: u32) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut p = Parser::new(msg);
    while let Some((f, v)) = p.next_field() {
        if f == field {
            if let Value::Bytes(b) = v {
                out.push(parse_container(b));
            }
        }
    }
    out
}

fn parse_container(msg: &[u8]) -> serde_json::Value {
    let name = get_string_field(msg, 1).unwrap_or_default();
    let image = get_string_field(msg, 2).unwrap_or_default();

    let mut command = Vec::new();
    let mut args = Vec::new();
    let mut env = Vec::new();
    let mut ports = Vec::new();

    let mut p = Parser::new(msg);
    while let Some((f, v)) = p.next_field() {
        match (f, v) {
            (3, Value::Bytes(b)) => command.push(serde_json::json!(lossy(b))),
            (4, Value::Bytes(b)) => args.push(serde_json::json!(lossy(b))),
            (6, Value::Bytes(b)) => {
                // EnvVar: name=1, value=2
                if let Some(k) = get_string_field(b, 1) {
                    let v = get_string_field(b, 2).unwrap_or_default();
                    env.push(serde_json::json!({"name": k, "value": v}));
                }
            }
            (7, Value::Bytes(b)) => {
                // ContainerPort: containerPort=1 (varint), protocol=4 (string)
                let port = get_varint_field(b, 1).unwrap_or(0);
                let protocol = get_string_field(b, 4).unwrap_or_else(|| "TCP".into());
                ports.push(serde_json::json!({"containerPort": port as i64, "protocol": protocol}));
            }
            _ => {}
        }
    }

    let mut c = serde_json::json!({ "name": name, "image": image });
    if !command.is_empty() {
        c["command"] = serde_json::json!(command);
    }
    if !args.is_empty() {
        c["args"] = serde_json::json!(args);
    }
    if !env.is_empty() {
        c["env"] = serde_json::json!(env);
    }
    if !ports.is_empty() {
        c["ports"] = serde_json::json!(ports);
    }
    c
}

// ── Low-level protobuf helpers ────────────────────────────────────────────────

fn get_bytes_field<'a>(msg: &'a [u8], target: u32) -> Option<&'a [u8]> {
    let mut p = Parser::new(msg);
    while let Some((f, v)) = p.next_field() {
        if f == target {
            if let Value::Bytes(b) = v {
                return Some(b);
            }
        }
    }
    None
}

fn get_string_field(msg: &[u8], target: u32) -> Option<String> {
    get_bytes_field(msg, target).map(|b| lossy(b))
}

fn get_varint_field(msg: &[u8], target: u32) -> Option<u64> {
    let mut p = Parser::new(msg);
    while let Some((f, v)) = p.next_field() {
        if f == target {
            if let Value::Varint(n) = v {
                return Some(n);
            }
        }
    }
    None
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

// ── Protobuf wire format parser ───────────────────────────────────────────────

enum Value<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed32(u32),
    Fixed64(u64),
}

struct Parser<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn next_field(&mut self) -> Option<(u32, Value<'a>)> {
        let tag = self.read_varint()?;
        let field = (tag >> 3) as u32;
        let wire = tag & 0x7;
        match wire {
            0 => {
                let n = self.read_varint()?;
                Some((field, Value::Varint(n)))
            }
            1 => {
                if self.pos + 8 > self.data.len() {
                    return None;
                }
                let bytes = &self.data[self.pos..self.pos + 8];
                self.pos += 8;
                let v = u64::from_le_bytes(bytes.try_into().ok()?);
                Some((field, Value::Fixed64(v)))
            }
            2 => {
                let len = self.read_varint()? as usize;
                if self.pos + len > self.data.len() {
                    return None;
                }
                let bytes = &self.data[self.pos..self.pos + len];
                self.pos += len;
                Some((field, Value::Bytes(bytes)))
            }
            5 => {
                if self.pos + 4 > self.data.len() {
                    return None;
                }
                let bytes = &self.data[self.pos..self.pos + 4];
                self.pos += 4;
                let v = u32::from_le_bytes(bytes.try_into().ok()?);
                Some((field, Value::Fixed32(v)))
            }
            _ => None,
        }
    }

    fn read_varint(&mut self) -> Option<u64> {
        let mut result: u64 = 0;
        let mut shift = 0u32;
        loop {
            if self.pos >= self.data.len() {
                return None;
            }
            let b = self.data[self.pos];
            self.pos += 1;
            result |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Some(result);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
    }
}
