//! Calls for the home computers screen: pairing, trust and the list of nearby machines.

use super::*;

impl Core {
    pub(super) fn nodes_call(&self, method: &str, args: &Value) -> Result<Value> {
        let r = crate::remote::get().ok_or_else(|| anyhow!("home computers are not available"))?;
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match method {
            "nodes.list" => {
                r.look_again_soon();
                Ok(r.list())
            }
            "nodes.seen" => {
                let addrs: Vec<String> = args.get("addrs").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
                r.seen(&s("name"), &s("id"), args.get("port").and_then(Value::as_u64).unwrap_or(0) as u16, &addrs, args.get("paused").and_then(Value::as_bool).unwrap_or(false));
                Ok(json!({}))
            }
            "nodes.gone" => {
                r.gone(&s("id"));
                Ok(json!({}))
            }
            "nodes.pair" => {
                let (addr, id) = (s("addr"), s("id"));
                r.pair((!addr.is_empty()).then_some(addr.as_str()), (!id.is_empty()).then_some(id.as_str()), &s("code"), &s("phone_name"))
            }
            "nodes.trust" => {
                let (addr, id) = (s("addr"), s("id"));
                r.trust((!addr.is_empty()).then_some(addr.as_str()), (!id.is_empty()).then_some(id.as_str()), &s("phone_name"))
            }
            "nodes.decide" => {
                r.decide(&s("fp"), &s("who"), args.get("allow").and_then(Value::as_bool).unwrap_or(false))?;
                Ok(json!({}))
            }
            "nodes.forget" => {
                r.forget(&s("fp"));
                Ok(json!({}))
            }
            "nodes.enable" => {
                r.set_enabled(args.get("on").and_then(Value::as_bool).unwrap_or(true));
                Ok(json!({}))
            }
            "nodes.network" => {
                r.network(args.get("up").and_then(Value::as_bool).unwrap_or(true));
                Ok(json!({}))
            }
            "nodes.refresh" => {
                r.refresh();
                Ok(r.list())
            }
            _ => Err(anyhow!("unknown call {method}")),
        }
    }
}
