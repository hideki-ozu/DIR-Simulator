from pathlib import Path
root=Path('/tmp/dir-opt-five-2026-10-08/candidate5/crates/dir-simulator/src/output')
p=root/'stream.rs';s=p.read_text()
pos=s.index('pub(super) struct Prepared {')
s=s[:pos]+'''/// Stable topology slots; names are looked up once per archived subject.
struct StatsTable {
    indices: BTreeMap<String, usize>,
    values: Vec<Stats>,
    all: usize,
}
impl StatsTable {
    fn new(prepared: &PreparedSimulation) -> Result<Self, Diagnostic> {
        let mut result = Self { indices: BTreeMap::new(), values: Vec::new(), all: 0 };
        for id in prepared.can.controllers.iter().map(|c| c.id.as_str())
            .chain(prepared.can.buses.iter().map(|b| b.id.as_str()))
            .chain(std::iter::once("$all")) {
            if !result.indices.contains_key(id) {
                let slot = result.values.len();
                reserve_vec(&mut result.values, 1, "output_stats_slots")?;
                result.values.push(Stats::default());
                result.indices.insert(id.to_owned(), slot);
            }
        }
        result.all = result.slot("$all")?;
        Ok(result)
    }
    fn slot(&self, name: &str) -> Result<usize, Diagnostic> {
        self.indices.get(name).copied()
            .ok_or_else(|| Diagnostic::output("Unknown archived statistics target"))
    }
}
impl std::ops::Index<&String> for StatsTable {
    type Output = Stats;
    fn index(&self, name: &String) -> &Self::Output {
        &self.values[self.indices[name]]
    }
}

'''+s[pos:]
s=s.replace("stats: &'a mut BTreeMap<String, Stats>","stats: &'a mut StatsTable").replace('stats: &BTreeMap<String, Stats>','stats: &StatsTable')
s=s.replace('''    for target in [&r.source, &r.bus, &String::from("$all")] {
        stats
            .entry(target.clone())
            .or_default()
            .request(&r.status)?;
    }''','''    let source_slot = stats.slot(&r.source)?;
    let bus_slot = stats.slot(&r.bus)?;
    let all_slot = stats.all;
    let request_slots = [source_slot, bus_slot, all_slot];
    for slot in request_slots { stats.values[slot].request(&r.status)?; }''')
s=s.replace('''        for target in [&r.source, &r.bus, &String::from("$all")] {
            stats.entry(target.clone()).or_default().delay(0, tx)?;
            stats.entry(target.clone()).or_default().delay(1, arb)?;
        }''','''        for slot in request_slots {
            stats.values[slot].delay(0, tx)?;
            stats.values[slot].delay(1, arb)?;
        }''')
s=s.replace('let bus = stats.entry(r.bus.clone()).or_default();','let bus = &mut stats.values[bus_slot];')
s=s.replace('for target in [&r.bus, &String::from("$all")] {','for slot in [bus_slot, all_slot] {')
s=s.replace('''            stats
                .entry(target.clone())
                .or_default()
                .delay(2, transfer)?;''','''            stats.values[slot].delay(2, transfer)?;''')
s=s.replace('''        for target in [&receiver.receiver, &r.bus, &String::from("$all")] {
            stats
                .entry(target.clone())
                .or_default()
                .receiver(&receiver.status)?;
        }''','''        let receiver_slot = stats.slot(&receiver.receiver)?;
        let receiver_slots = [receiver_slot, bus_slot, all_slot];
        for slot in receiver_slots { stats.values[slot].receiver(&receiver.status)?; }''')
s=s.replace('let receiver_stats = stats.entry(receiver.receiver.clone()).or_default();','let receiver_stats = &mut stats.values[receiver_slot];')
s=s.replace('for target in [&receiver.receiver, &r.bus, &String::from("$all")] {','for slot in receiver_slots {')
s=s.replace('''                stats
                    .entry(target.clone())
                    .or_default()
                    .delay(3, delivery)?;''','''                stats.values[slot].delay(3, delivery)?;''')
s=s.replace('''    let mut stats = BTreeMap::new();
    for c in &prepared.can.controllers {
        stats.insert(c.id.clone(), Stats::default());
    }
    for bus in &prepared.can.buses {
        stats.insert(bus.id.clone(), Stats::default());
    }
    stats.insert("$all".into(), Stats::default());''','''    let mut stats = StatsTable::new(prepared)?;''')
assert 'stats.entry' not in s
assert 'String::from("$all")' not in s
p.write_text(s)
# Reuse existing immutable fixtures without copying large generated results.
base=Path('/tmp/dir-opt-five-2026-10-08')
for work in [base/'baseline-source',*[base/f'candidate{n}' for n in range(1,6)]]:
 for name in ['docs','examples','scripts','tests']:
  dest=work/name
  if not dest.exists(): dest.symlink_to(Path('/home/hideki/DIR-Simulator')/name,target_is_directory=True)
