use std::cmp::{Ord, PartialOrd};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use capstone::Instructions;
use crossbeam::queue::SegQueue;
//use indexmap::map::IndexMap;
//use indexmap::set::IndexSet;
use hashbrown::{HashMap, HashSet};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
pub use unicorn::unicorn_const::Error as UCError;
use unicorn::Cpu;

use crate::emulator::loader;
use crate::emulator::loader::{get_static_memory_image, Seg};
use crate::emulator::register_pattern::{Register, RegisterState};

// TODO: why store the size at all, if you're just going to
// throw it away?
#[derive(Copy, Clone, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize, Hash)]
pub struct Block {
    pub entry: u64,
    pub size: usize,
    //pub code: Vec<u8>,
}

impl Block {
    pub fn get_code(&self) -> &'static [u8] {
        let memory = loader::get_static_memory_image();
        memory
            .try_dereference(self.entry, None)
            .map(|b| &b[..self.size])
            .unwrap()
    }

    pub fn disassemble(&self) -> Instructions<'_> {
        let memory = loader::get_static_memory_image();
        memory
            .disassemble(self.entry, self.size, None)
            .expect("Failed to disassemble basic block")
    }
}

impl fmt::Debug for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[BLOCK 0x{:08x} - 0x{:08x}]",
            self.entry,
            self.entry + self.size as u64
        )
    }
}

pub struct Profiler<C: Cpu<'static>> {
    /// The Arc<RwLock<_>> fields need to be writeable for the unicorn callbacks.
    pub block_log: Arc<SegQueue<Block>>,
    pub gadget_log: Arc<SegQueue<u64>>,
    //Arc<RwLock<Vec<u64>>>,
    /// These fields are written to after the emulation has finished.
    pub written_memory: Vec<Seg>,
    pub write_log: Arc<SegQueue<MemLogEntry>>,
    //Arc<RwLock<Vec<MemLogEntry>>>,
    pub cpu_error: Option<unicorn::Error>,
    pub emulation_time: Duration,
    pub registers: HashMap<Register<C>, u64>,
    registers_to_read: Vec<Register<C>>,
    pub input: HashMap<Register<C>, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Profile {
    pub paths: Vec<Vec<Block>>,
    //PrefixSet<Block>,
    // TODO: cpu_errors should be a vector of Option<usize>
    pub cpu_errors: Vec<Option<UCError>>,
    pub emulation_times: Vec<Duration>,
    pub registers: Vec<RegisterState>,
    pub gadgets_executed: Vec<HashSet<u64>>,
    #[cfg(not(feature = "full_dump"))]
    #[serde(skip)]
    pub writeable_memory: Vec<Vec<Seg>>,
    pub write_logs: Vec<Vec<MemLogEntry>>,
    pub executable: bool,
}

// FIXME: refactor so that we don't have any code duplication between
// this method and collate. Or just get rid of collate entirely, I guess.

impl<C: 'static + Cpu<'static>> From<Profiler<C>> for Profile {
    fn from(p: Profiler<C>) -> Self {
        let mut paths = Vec::new(); // PrefixSet::new();
        let mut cpu_errors = Vec::new();
        let mut computation_times = Vec::new();
        let mut register_maps = Vec::new();
        let mut gadgets_executed = Vec::new();
        let writeable_memory_regions = Vec::new();
        let mut write_logs = Vec::new();

        let Profiler {
            block_log,
            write_log,
            cpu_error,
            emulation_time,
            registers,
            gadget_log,
            written_memory,
            ..
        } = p;
        paths.push(segqueue_to_vec(block_log));
        let mut executed = HashSet::new();
        while let Ok(g) = gadget_log.pop() {
            executed.insert(g);
        }
        gadgets_executed.push(executed);
        cpu_errors.push(cpu_error);
        computation_times.push(emulation_time);
        register_maps.push(RegisterState::new::<C>(&registers, Some(&written_memory)));

        write_logs.push(segqueue_to_vec(write_log));

        Self {
            paths,
            cpu_errors,
            emulation_times: computation_times,
            gadgets_executed,
            registers: register_maps,
            writeable_memory: writeable_memory_regions,
            write_logs,
            executable: true,
        }
    }
}

fn segqueue_to_vec<T>(sq: Arc<SegQueue<T>>) -> Vec<T> {
    let mut v = vec![];
    while let Ok(x) = sq.pop() {
        v.push(x)
    }
    //log::debug!("vec of {} blocks", v.len());
    v
}

impl Profile {
    // combine the information in two different profiles by absorbing the second
    // into the first
    pub fn absorb(&mut self, other: Self) {
        let Self {
            paths,
            cpu_errors,
            emulation_times,
            registers,
            gadgets_executed,
            writeable_memory,
            write_logs,
            executable,
        } = other;

        self.paths.extend(paths.into_iter());
        self.cpu_errors.extend(cpu_errors.into_iter());
        self.emulation_times.extend(emulation_times.into_iter());
        self.registers.extend(registers.into_iter());
        self.gadgets_executed.extend(gadgets_executed.into_iter());
        self.writeable_memory.extend(writeable_memory.into_iter());
        self.write_logs.extend(write_logs.into_iter());
        self.executable &= executable;
    }

    pub fn collate<C: 'static + Cpu<'static>>(profilers: Vec<Profiler<C>>) -> Self {
        //let mut write_trie = Trie::new();
        let mut paths = Vec::new(); // PrefixSet::new();
        let mut cpu_errors = Vec::new();
        let mut computation_times = Vec::new();
        let mut register_maps = Vec::new();
        let mut gadgets_executed = Vec::new();
        let writeable_memory_regions = Vec::new();
        let mut write_logs = Vec::new();

        for Profiler {
            block_log,
            write_log,
            cpu_error,
            emulation_time,
            registers,
            gadget_log,
            written_memory,
            ..
        } in profilers.into_iter()
        {
            paths.push(segqueue_to_vec(block_log));
            let mut executed = HashSet::new();
            while let Ok(g) = gadget_log.pop() {
                executed.insert(g);
            }
            gadgets_executed.push(executed);
            // NOTE: changing gadgets_executed to a vec of hashsets

            cpu_errors.push(cpu_error);
            computation_times.push(emulation_time);
            // FIXME: use a different data type for output states.
            register_maps.push(RegisterState::new::<C>(&registers, Some(&written_memory)));

            write_logs.push(segqueue_to_vec(write_log));
        }

        Self {
            paths,
            cpu_errors,
            emulation_times: computation_times,
            gadgets_executed,
            registers: register_maps,
            writeable_memory: writeable_memory_regions,
            write_logs,
            executable: true,
        }
    }

    pub fn avg_emulation_micros(&self) -> f64 {
        self.emulation_times.iter().sum::<Duration>().as_micros() as f64
            / self.emulation_times.len() as f64
    }

    pub fn basic_block_path_iterator(&self) -> impl Iterator<Item = &Vec<Block>> + '_ {
        self.paths.iter()
    }

    pub fn disas_paths(&self) -> impl Iterator<Item = String> + '_ {
        self.paths.iter().map(move |path| {
            path.par_iter()
                .map(|b| {
                    let prefix = if self.was_this_executed(b.entry) {
                        "----\n"
                    } else {
                        ""
                    };
                    format!("{}{}\n", prefix, b.disassemble())
                })
                .collect::<String>()
        })
    }

    pub fn addresses_written_to(&self) -> HashSet<u64> {
        let mut set = HashSet::new();
        self.write_logs.iter().flatten().for_each(|entry| {
            for i in 0..entry.num_bytes_written {
                set.insert(entry.address + i as u64);
            }
        });
        set
    }

    pub fn mem_write_ratio(&self) -> f64 {
        let memory = get_static_memory_image();
        let size_of_writeable = memory.size_of_writeable_memory();
        let bytes_written = self.addresses_written_to().len();
        bytes_written as f64 / size_of_writeable as f64
    }

    /// Given a word `w`, return a vector of entries that show
    /// that word was written. If the word was not written, return
    /// an empty vector.
    ///
    /// This method flattens the write_log, and doesn't distinguish
    /// between inputs. This is easily changed, if we end up needing
    /// to make that distinction in the future.
    pub fn was_this_written(&self, w: u64) -> Vec<&MemLogEntry> {
        self.write_logs
            .iter()
            .flatten()
            .filter(|entry| {
                entry.value == w // could return hamming score instead
            })
            .collect()
    }

    pub fn was_this_executed(&self, w: u64) -> bool {
        for gads in self.gadgets_executed.iter() {
            if gads.contains(&w) {
                return true;
            }
        }
        false
    }
}

impl<C: 'static + Cpu<'static>> From<Vec<Profiler<C>>> for Profile {
    fn from(v: Vec<Profiler<C>>) -> Self {
        Self::collate(v)
    }
}

impl<C: Cpu<'static>> fmt::Debug for Profiler<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "registers: {:?}; ", self.registers)?;
        write!(f, "cpu_error: {:?}; ", self.cpu_error)?;
        write!(
            f,
            "computation_time: {} μs; ",
            self.emulation_time.as_micros()
        )
    }
}

impl<C: Cpu<'static>> Profiler<C> {
    pub fn new(output_registers: &[Register<C>], input: &HashMap<Register<C>, u64>) -> Self {
        Self {
            registers_to_read: output_registers.to_vec(),
            input: input.clone(),
            ..Default::default()
        }
    }

    pub fn read_registers(&mut self, emu: &mut C) {
        for r in &self.registers_to_read {
            let val = emu.reg_read(*r).expect("Failed to read register!");
            self.registers.insert(*r, val);
        }
    }

    pub fn register(&self, reg: Register<C>) -> Option<u64> {
        self.registers.get(&reg).cloned()
    }

    pub fn set_error(&mut self, error: unicorn::Error) {
        self.cpu_error = Some(error)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize, Hash)]
pub struct MemLogEntry {
    pub program_counter: u64,
    pub address: u64,
    pub num_bytes_written: usize,
    pub value: u64,
}

impl<C: Cpu<'static>> Default for Profiler<C> {
    fn default() -> Self {
        Self {
            write_log: Arc::new(SegQueue::new()), //Arc::new(RwLock::new(Vec::default())),
            input: HashMap::default(),
            registers: HashMap::default(),
            cpu_error: None,
            registers_to_read: Vec::new(),
            emulation_time: Duration::default(),
            block_log: Arc::new(SegQueue::new()),
            gadget_log: Arc::new(SegQueue::new()), //Arc::new(RwLock::new(Vec::new())),
            written_memory: vec![],
        }
    }
}

pub trait HasProfile {
    fn profile(&self) -> Option<&Profile>;

    fn add_profile(&mut self, profile: Profile);
}

#[cfg(test)]
mod test {
    use unicorn::CpuX86;

    use super::*;

    macro_rules! segqueue {
        ($($x:expr,)*) => {
            {
                let q = SegQueue::new();
                $(
                   q.push($x);
                )*
                q
            }
        }
    }
    #[test]
    fn test_collate() {
        let profilers: Vec<Profiler<CpuX86<'_>>> = vec![
            Profiler {
                block_log: Arc::new(segqueue![
                    Block { entry: 1, size: 2 },
                    Block { entry: 3, size: 4 },
                ]),
                cpu_error: None,
                emulation_time: Default::default(),
                registers: HashMap::new(),
                ..Default::default()
            },
            Profiler {
                block_log: Arc::new(segqueue![
                    Block { entry: 1, size: 2 },
                    Block { entry: 6, size: 6 },
                ]),
                cpu_error: None,
                emulation_time: Default::default(),
                registers: HashMap::new(),
                ..Default::default()
            },
        ];

        let profile: Profile = profilers.into();

        println!(
            "size of profile in mem: {}",
            std::mem::size_of_val(&profile.paths)
        );
    }
}
