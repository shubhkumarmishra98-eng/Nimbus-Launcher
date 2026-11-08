use crate::instances::load_instance;
use crate::util::*;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// Minimal NBT model — enough to round-trip Minecraft's uncompressed servers.dat.
#[derive(Clone, Debug)]
enum Nbt {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    Str(String),
    List(u8, Vec<Nbt>),
    Compound(Vec<(String, Nbt)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .p
            .checked_add(n)
            .ok_or_else(|| anyhow!("bad NBT length"))?;
        if end > self.b.len() {
            return Err(anyhow!("truncated NBT"));
        }
        let s = &self.b[self.p..end];
        self.p = end;
        Ok(s)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn count(&mut self) -> Result<usize> {
        let n = i32::from_be_bytes(self.take(4)?.try_into()?);
        if n < 0 {
            return Err(anyhow!("negative NBT length"));
        }
        Ok(n as usize)
    }
}

fn read_str(r: &mut Rd) -> Result<String> {
    let n = u16::from_be_bytes(r.take(2)?.try_into()?) as usize;
    Ok(String::from_utf8_lossy(r.take(n)?).to_string())
}

fn read_payload(r: &mut Rd, ty: u8, depth: u32) -> Result<Nbt> {
    if depth > 64 {
        return Err(anyhow!("NBT nested too deeply"));
    }
    Ok(match ty {
        1 => Nbt::Byte(r.byte()? as i8),
        2 => Nbt::Short(i16::from_be_bytes(r.take(2)?.try_into()?)),
        3 => Nbt::Int(i32::from_be_bytes(r.take(4)?.try_into()?)),
        4 => Nbt::Long(i64::from_be_bytes(r.take(8)?.try_into()?)),
        5 => Nbt::Float(f32::from_be_bytes(r.take(4)?.try_into()?)),
        6 => Nbt::Double(f64::from_be_bytes(r.take(8)?.try_into()?)),
        7 => {
            let n = r.count()?;
            Nbt::ByteArray(r.take(n)?.iter().map(|b| *b as i8).collect())
        }
        8 => Nbt::Str(read_str(r)?),
        9 => {
            let t = r.byte()?;
            let n = r.count()?;
            let mut v = vec![];
            for _ in 0..n {
                v.push(read_payload(r, t, depth + 1)?);
            }
            Nbt::List(t, v)
        }
        10 => {
            let mut v = vec![];
            loop {
                let t = r.byte()?;
                if t == 0 {
                    break;
                }
                let name = read_str(r)?;
                v.push((name, read_payload(r, t, depth + 1)?));
            }
            Nbt::Compound(v)
        }
        11 => {
            let n = r.count()?;
            let mut v = vec![];
            for _ in 0..n {
                v.push(i32::from_be_bytes(r.take(4)?.try_into()?));
            }
            Nbt::IntArray(v)
        }
        12 => {
            let n = r.count()?;
            let mut v = vec![];
            for _ in 0..n {
                v.push(i64::from_be_bytes(r.take(8)?.try_into()?));
            }
            Nbt::LongArray(v)
        }
        _ => return Err(anyhow!("unknown NBT tag {ty}")),
    })
}

fn tag_id(n: &Nbt) -> u8 {
    match n {
        Nbt::Byte(_) => 1,
        Nbt::Short(_) => 2,
        Nbt::Int(_) => 3,
        Nbt::Long(_) => 4,
        Nbt::Float(_) => 5,
        Nbt::Double(_) => 6,
        Nbt::ByteArray(_) => 7,
        Nbt::Str(_) => 8,
        Nbt::List(..) => 9,
        Nbt::Compound(_) => 10,
        Nbt::IntArray(_) => 11,
        Nbt::LongArray(_) => 12,
    }
}

fn write_str(o: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    o.extend((b.len() as u16).to_be_bytes());
    o.extend(b);
}

fn write_payload(o: &mut Vec<u8>, n: &Nbt) {
    match n {
        Nbt::Byte(v) => o.push(*v as u8),
        Nbt::Short(v) => o.extend(v.to_be_bytes()),
        Nbt::Int(v) => o.extend(v.to_be_bytes()),
        Nbt::Long(v) => o.extend(v.to_be_bytes()),
        Nbt::Float(v) => o.extend(v.to_be_bytes()),
        Nbt::Double(v) => o.extend(v.to_be_bytes()),
        Nbt::ByteArray(v) => {
            o.extend((v.len() as i32).to_be_bytes());
            o.extend(v.iter().map(|b| *b as u8));
        }
        Nbt::Str(s) => write_str(o, s),
        Nbt::List(t, v) => {
            o.push(*t);
            o.extend((v.len() as i32).to_be_bytes());
            for x in v {
                write_payload(o, x);
            }
        }
        Nbt::Compound(v) => {
            for (k, x) in v {
                o.push(tag_id(x));
                write_str(o, k);
                write_payload(o, x);
            }
            o.push(0);
        }
        Nbt::IntArray(v) => {
            o.extend((v.len() as i32).to_be_bytes());
            for x in v {
                o.extend(x.to_be_bytes());
            }
        }
        Nbt::LongArray(v) => {
            o.extend((v.len() as i32).to_be_bytes());
            for x in v {
                o.extend(x.to_be_bytes());
            }
        }
    }
}

fn path_of(id: &str) -> PathBuf {
    game_dir(id).join("servers.dat")
}

fn empty_root() -> Nbt {
    Nbt::Compound(vec![("servers".to_string(), Nbt::List(10, vec![]))])
}

fn load_root(p: &PathBuf) -> Result<Nbt> {
    if !p.exists() {
        return Ok(empty_root());
    }
    let bytes = std::fs::read(p)?;
    let mut r = Rd { b: &bytes, p: 0 };
    if r.byte()? != 10 {
        return Err(anyhow!("servers.dat is not an NBT compound"));
    }
    let _root_name = read_str(&mut r)?;
    read_payload(&mut r, 10, 0)
}

fn get_str(c: &[(String, Nbt)], k: &str) -> String {
    c.iter()
        .find(|e| e.0 == k)
        .and_then(|e| match &e.1 {
            Nbt::Str(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn set_str(c: &mut Vec<(String, Nbt)>, k: &str, v: &str) {
    if let Some(e) = c.iter_mut().find(|e| e.0 == k) {
        e.1 = Nbt::Str(v.to_string());
    } else {
        c.push((k.to_string(), Nbt::Str(v.to_string())));
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ServerEntry {
    pub name: String,
    pub ip: String,
}

#[tauri::command]
pub fn servers_list(instance_id: String) -> Result<Vec<ServerEntry>, String> {
    let run = || -> Result<Vec<ServerEntry>> {
        load_instance(&instance_id)?;
        let root = load_root(&path_of(&instance_id))?;
        let mut out = vec![];
        if let Nbt::Compound(items) = &root {
            if let Some((_, Nbt::List(_, list))) = items.iter().find(|e| e.0 == "servers") {
                for s in list {
                    if let Nbt::Compound(c) = s {
                        out.push(ServerEntry {
                            name: get_str(c, "name"),
                            ip: get_str(c, "ip"),
                        });
                    }
                }
            }
        }
        Ok(out)
    };
    run().map_err(es)
}

#[tauri::command]
pub fn servers_save(instance_id: String, servers: Vec<ServerEntry>) -> Result<(), String> {
    let run = || -> Result<()> {
        load_instance(&instance_id)?;
        let p = path_of(&instance_id);
        let mut root = load_root(&p).unwrap_or_else(|_| empty_root());
        // keep extra per-server data (icons, flags) for entries whose address is unchanged
        let mut old: Vec<Vec<(String, Nbt)>> = vec![];
        if let Nbt::Compound(items) = &root {
            if let Some((_, Nbt::List(_, list))) = items.iter().find(|e| e.0 == "servers") {
                for s in list {
                    if let Nbt::Compound(c) = s {
                        old.push(c.clone());
                    }
                }
            }
        }
        let mut list: Vec<Nbt> = vec![];
        for s in servers.iter().filter(|s| !s.ip.trim().is_empty()) {
            let ip = s.ip.trim();
            let mut c = old
                .iter()
                .find(|c| get_str(c, "ip") == ip)
                .cloned()
                .unwrap_or_default();
            set_str(&mut c, "name", if s.name.trim().is_empty() { ip } else { s.name.trim() });
            set_str(&mut c, "ip", ip);
            list.push(Nbt::Compound(c));
        }
        if let Nbt::Compound(items) = &mut root {
            items.retain(|e| e.0 != "servers");
            items.push(("servers".to_string(), Nbt::List(10, list)));
        }
        let mut out = vec![10u8];
        write_str(&mut out, "");
        write_payload(&mut out, &root);
        if p.exists() {
            let _ = std::fs::copy(&p, p.with_extension("dat_old"));
        }
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&p, out)?;
        Ok(())
    };
    run().map_err(es)
}
