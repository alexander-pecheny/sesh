//! Two devices syncing one Vault as Shared/Vault.swift does, through random pushes, follows,
//! reconnects, lost replies and a Host erased and made again, checked for lost or doubled records.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{json, Value};

use super::{Change, Record, Vault};

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

struct Host {
    root: PathBuf,
    made: usize,
    vault: Vault,
}

impl Host {
    fn new(root: PathBuf) -> Self {
        let _ = std::fs::remove_dir_all(&root);
        let vault = Vault::open(root.join("0"), true).unwrap();
        Host { root, made: 0, vault }
    }

    fn erase(&mut self) {
        self.made += 1;
        self.vault = Vault::open(self.root.join(self.made.to_string()), true).unwrap();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, PartialEq, Debug)]
struct Queued {
    id: String,
    kind: String,
    body: Value,
    base: i64,
}

impl Queued {
    fn change(&self) -> Change {
        Change { id: self.id.clone(), kind: self.kind.clone(), body: self.body.clone(), base: self.base, deleted: false }
    }
}

#[derive(Default)]
struct Device {
    name: String,
    seq: i64,
    identity: Option<String>,
    records: HashMap<String, Record>,
    /// The Host's newest record of each id follow passed over while a change to it was queued.
    shadows: HashMap<String, Record>,
    queue: Vec<Queued>,
    batch: Option<(String, Vec<Queued>)>,
    batches: usize,
    wrote: HashMap<String, Value>,
}

impl Device {
    fn write(&mut self, id: String, body: Value) {
        let base = self.records.get(&id).map_or(0, |record| record.seq);
        let record = Record { id: id.clone(), kind: "entry".into(), body: body.clone(), seq: base, deleted: false };
        self.records.insert(id.clone(), record);
        self.queue.retain(|queued| queued.id != id);
        self.queue.push(Queued { id: id.clone(), kind: "entry".into(), body: body.clone(), base });
        self.wrote.insert(id, body);
    }

    /// One push; `idempotent` false sends each try as a new batch, as Sesh did before.
    fn flush(&mut self, host: &mut Host, random: &mut Random, lossy: bool, idempotent: bool) {
        if self.queue.is_empty() {
            return;
        }
        let (batch, sent) = match self.batch.clone().filter(|_| idempotent) {
            Some(batch) => batch,
            None => {
                self.batches += 1;
                (format!("{}-{}", self.name, self.batches), self.queue.clone())
            }
        };
        self.batch = Some((batch.clone(), sent.clone()));
        if lossy && random.chance(10) {
            return;
        }
        let (records, _) = host.vault.push(Some(&batch), sent.iter().map(Queued::change).collect()).unwrap();
        // An answer from another Vault than the one followed waits for the reconnect it calls for.
        if lossy && random.chance(25) || !self.connected(host) {
            return;
        }
        self.batch = None;
        for change in &sent {
            if let Some(index) = self.queue.iter().position(|queued| queued.id == change.id && queued.body == change.body) {
                self.queue.remove(index);
            }
        }
        for record in records {
            if let Some(queued) = self.queue.iter_mut().find(|queued| queued.id == record.id) {
                queued.base = record.seq;
                if let Some(local) = self.records.get_mut(&record.id) {
                    local.seq = record.seq;
                }
            } else {
                let newest = match self.shadows.remove(&record.id) {
                    Some(shadow) if shadow.seq > record.seq => shadow,
                    _ => record,
                };
                self.records.insert(newest.id.clone(), newest);
            }
        }
    }

    /// Follow's lines from the cursor, cut short at random when the connection drops.
    fn follow(&mut self, host: &Host, random: &mut Random, lossy: bool) {
        if !self.connected(host) {
            return;
        }
        let (records, head) = host.vault.pull(self.seq).unwrap();
        let shown = if lossy { random.below(records.len() as u64 + 2) as usize } else { usize::MAX };
        for record in records.iter().take(shown) {
            self.seq = self.seq.max(record.seq);
            if self.queue.iter().any(|queued| queued.id == record.id) {
                self.shadows.insert(record.id.clone(), record.clone());
            } else {
                self.records.insert(record.id.clone(), record.clone());
            }
        }
        if shown > records.len() {
            self.seq = self.seq.max(head);
        }
    }

    /// Whether this device last connected to the Vault the Host now holds; a follow from before
    /// it was erased hears nothing more.
    fn connected(&self, host: &Host) -> bool {
        self.identity.as_ref() == Some(&host.vault.id)
    }

    /// `vault init` on connecting: a Host behind this copy, or another Vault, is refilled.
    fn reconnect(&mut self, host: &Host) {
        let id = host.vault.id.clone();
        if host.vault.head().unwrap() < self.seq || self.identity.as_ref().is_some_and(|known| *known != id) {
            let (all, _) = host.vault.pull(0).unwrap();
            let have: HashMap<&str, &Record> = all.iter().map(|record| (record.id.as_str(), record)).collect();
            for (id, record) in &self.records {
                let lacks = have.get(id.as_str()).is_none_or(|had| had.body != record.body && had.body["edited"].as_i64() < record.body["edited"].as_i64());
                if lacks && !self.queue.iter().any(|queued| queued.id == *id) {
                    self.queue.push(Queued { id: id.clone(), kind: record.kind.clone(), body: record.body.clone(), base: 0 });
                }
            }
            self.seq = 0;
            self.shadows.clear();
        }
        self.identity = Some(id);
    }
}

fn run(seed: u64, idempotent: bool) -> Result<(), String> {
    let mut random = Random(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut host = Host::new(std::env::temp_dir().join(format!("sesh-sync-{}-{seed}-{idempotent}", std::process::id())));
    let mut devices: Vec<Device> = ["mac", "phone"].iter().map(|name| Device { name: name.to_string(), ..Default::default() }).collect();
    let (mut clock, mut erased) = (0, false);
    for _ in 0..200 {
        let device = &mut devices[random.below(2) as usize];
        match random.below(100) {
            0..=29 => {
                clock += 1;
                let id = format!("{}-{}", device.name, random.below(5));
                device.write(id, json!({"text": format!("{} {clock}", device.name), "edited": clock}));
            }
            30..=59 => device.flush(&mut host, &mut random, true, idempotent),
            60..=84 => device.follow(&host, &mut random, true),
            // A third of the runs erase the Host now and then.
            98.. if seed % 3 == 0 => {
                host.erase();
                erased = true;
            }
            _ => device.reconnect(&host),
        }
    }
    for _ in 0..2 {
        for device in &mut devices {
            device.reconnect(&host);
            while !device.queue.is_empty() {
                device.flush(&mut host, &mut random, false, idempotent);
            }
            device.follow(&host, &mut random, false);
        }
    }
    let (stored, _) = host.vault.pull(0).unwrap();
    let wrote: HashMap<&String, &Value> = devices.iter().flat_map(|device| &device.wrote).collect();
    // Two devices refilling an erased Host may meet on one record, and the older text is kept.
    if let Some(conflict) = stored.iter().find(|record| record.kind == "conflict" && !erased) {
        return Err(format!("seed {seed}: a device's own edit came back as a conflict copy {}", conflict.body));
    }
    for (id, body) in &wrote {
        let found = stored.iter().find(|record| record.id == **id).map(|record| &record.body);
        if found != Some(body) {
            return Err(format!("seed {seed}: the Host has {found:?} for {id}, not the last write {body}"));
        }
    }
    let entries = stored.iter().filter(|record| record.kind != "conflict").count();
    if entries != wrote.len() {
        return Err(format!("seed {seed}: the Host holds {entries} records for {} written", wrote.len()));
    }
    let truth: HashMap<String, Record> = stored.into_iter().map(|record| (record.id.clone(), record)).collect();
    for device in &devices {
        if device.records != truth {
            let differ: Vec<_> = truth.values().filter(|record| device.records.get(&record.id) != Some(record)).collect();
            return Err(format!("seed {seed}: the {}'s copy differs from the Host on {differ:?}", device.name));
        }
    }
    Ok(())
}

#[test]
fn two_devices_lose_and_double_no_record_through_lost_replies_reconnects_and_an_erased_host() {
    for seed in 0..200 {
        run(seed, true).unwrap();
    }
}

#[test]
fn without_remembered_batches_a_lost_reply_makes_a_conflict_copy_of_a_devices_own_edit() {
    assert!((0..200).any(|seed| run(seed, false).is_err()));
}
