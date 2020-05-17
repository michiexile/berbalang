use std::iter;
use std::iter::Iterator;

use log;
use rand::distributions::Alphanumeric;
use rand::prelude::*;

use crate::evolution::*;
use crate::observer::Observe;

#[derive(Clone, Debug, Default)]
pub struct ObserverConfig {
    pub window_size: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub mut_rate: f32,
    pub init_len: usize,
    pub pop_size: usize,
    pub tournament_size: usize,
    pub num_offspring: usize,
    pub target: String,
    pub observer: ObserverConfig,
}

impl Config {
    pub fn assert_invariants(&self) {
        assert!(self.tournament_size >= self.num_offspring + 2);
    }
}

#[derive(Clone, Debug, Default)]
pub struct Genome {
    genes: String,
    fitness: Option<Fitness>,
}

impl Genome {
    pub fn new(len: usize) -> Self {
        let mut rng = thread_rng();
        let s: String = iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .take(len)
            .collect();
        Self {
            genes: s,
            fitness: None,
        }
    }

    pub fn len(&self) -> usize {
        self.genes.len()
    }

    pub fn mate(&self, other: &Self, params: &Config) -> Vec<Self> {
        log::debug!("Mating {:?} and {:?}", self, other);
        let mut offspring = self.crossover(other);
        let mut rng = thread_rng();
        for child in offspring.iter_mut() {
            if rng.gen::<f32>() < params.mut_rate {
                child.mutate();
            }
        }
        log::debug!("Offspring: {:?}", offspring);
        offspring
    }

    pub fn fitter_than(&self, other: &Self) -> bool {
        match (self.fitness, other.fitness) {
            (Some(a), Some(b)) if a < b => true,
            _ => false,
        }
    }
}

impl Genotype for Genome {
    fn crossover(&self, mate: &Self) -> Vec<Self> {
        let mut rng = thread_rng();
        let split_m: usize = rng.gen::<usize>() % self.len();
        let split_f: usize = rng.gen::<usize>() % mate.len();
        let (m1, m2) = self.genes.split_at(split_m);
        let (f1, f2) = mate.genes.split_at(split_f);
        vec![
            Genome {
                genes: format!("{}{}", m1, f2),
                fitness: None,
            },
            Genome {
                genes: format!("{}{}", m2, f1),
                fitness: None,
            },
        ]
    }

    fn mutate(&mut self) {
        let mut rng: ThreadRng = thread_rng();
        let i: usize = rng.gen::<usize>() % self.len();
        unsafe {
            let bytes = self.genes.as_bytes_mut();
            let mut c: u8 = 0;
            while c < 0x20 || 0x7E < c {
                c = rng.gen::<u8>();
            }
            bytes[i] = c;
        }
    }
}

pub type Fitness = usize;

impl ScoreMut for Genome {
    type Params = Config;
    type Fitness = Fitness;

    fn score_mut(&mut self, params: &Self::Params) -> Self::Fitness {
        // let's have the fitness just be the levenshtein distance
        if self.fitness.is_none() {
            self.fitness = Some(distance::levenshtein(&self.genes, &params.target))
        };
        self.fitness.expect("unreachable")
    }
}

#[derive(Debug)]
pub struct Epoch {
    pub population: Vec<Genome>,
    pub config: Config,
    pub best: Option<Genome>,
    pub iteration: usize,
}

impl Epoch {
    pub fn new(config: Config) -> Self {
        let population = iter::repeat(())
            .map(|()| Genome::new(config.init_len))
            .take(config.pop_size)
            .collect();
        Self {
            population,
            config,
            best: None,
            iteration: 0,
        }
    }

    pub fn update_best(&mut self, champ: &Genome) {
        match self.best {
            Some(ref best) if champ.fitter_than(best) => {
                self.best = Some(champ.clone());
                log::info!("[{}]\tnew champ {:?}", self.iteration, champ)
            }
            None => {
                self.best = Some(champ.clone());
                log::info!("[{}]\tnew champ {:?}", self.iteration, champ)
            }
            _ => {}
        }
    }
}

impl Epochal for Epoch {
    type Observer = observation::Observer;
    type Evaluator = evaluation::Evaluator;

    fn evolve(mut self, observer: &Self::Observer, evaluator: &Self::Evaluator) -> Self {
        // draw 4 unique lots from the hat
        let tournament_size = self.config.tournament_size;
        let mut population = std::mem::take(&mut self.population);
        let mut rng = thread_rng();
        // shuffle is O(1)!
        population.shuffle(&mut rng);
        let mut combatants = Vec::new();
        for _ in 0..tournament_size {
            if let Some(combatant) = population.pop() {
                let scored = evaluator.evaluate(combatant);
                observer.observe(scored.clone());
                combatants.push(scored)
            } else {
                log::error!("Population empty!");
            }
        }
        // none of the fitnesses will be None, here, so we can freely compare them
        combatants.sort_by(|a, b| a.fitness.cmp(&b.fitness));
        self.update_best(&combatants[0]);

        // kill one off for every offspring to be produced
        for _ in 0..self.config.num_offspring {
            let _ = combatants.pop();
        }

        // replace the combatants that will neither breed nor die
        let bystanders = self.config.tournament_size - (self.config.num_offspring + 2);
        for _ in 0..bystanders {
            if let Some(c) = combatants.pop() {
                population.push(c);
            }
        }

        let mother = combatants.pop().unwrap();
        let father = combatants.pop().unwrap();
        let offspring: Vec<Genome> = mother.mate(&father, &self.config);

        // return everyone to the population
        population.push(mother);
        population.push(father);
        for child in offspring.into_iter() {
            population.push(child)
        }

        /*
                let mut lots: Vec<usize> = (0..population.len()).collect::<Vec<usize>>();
                lots.shuffle(&mut thread_rng());
                let fitnesses: Vec<_> = lots
                    .iter()
                    .take(4)
                    .map(|i| population[*i].score_mut(&self.config))
                    .collect();
                let mut indexed: Vec<(Fitness, usize)> =
                    fitnesses.into_iter().zip(lots.into_iter()).collect();
                // Rank lots by the fitnesses of the corresponding genomes
                indexed.sort();

                for (_, i) in indexed.iter() {
                    observer.observe(population[*i].clone())
                        .expect("observation failed");
                }

                let champ = &population[indexed[0].1];
                match self.best {
                    Some(ref best) if champ.fitter_than(best) => {
                        self.best = Some(champ.clone());
                        log::debug!("[{}]\tnew champ {:?}", self.iteration, champ)
                    }
                    None => {
                        self.best = Some(champ.clone());
                        log::debug!("[{}]\tnew champ {:?}", self.iteration, champ)
                    }
                    _ => {}
                }

                let winners: Vec<usize> = indexed[0..2].iter().map(|(_, i)| *i).collect();
                let dead: Vec<usize> = indexed[indexed.len() - 1..]
                    .iter()
                    .map(|(_, i)| *i)
                    .collect();

                let mut offspring: Vec<Genome> =
                    population[winners[0]].mate(&population[winners[1]], &self.config);

                // now replace the dead with the offspring
                population[dead[0]] = offspring.pop().expect("where did they go?");
                population[dead[0]] = offspring.pop().expect("where did they go?");
        */
        Self {
            population,
            config: std::mem::take(&mut self.config),
            best: std::mem::take(&mut self.best),
            iteration: self.iteration + 1,
        }
    }
}

pub fn run(config: Config) -> Option<Genome> {
    let mut world = Epoch::new(config);
    let observer = observation::Observer::spawn(&world.config);
    let evaluator = evaluation::Evaluator::spawn(&world.config);

    loop {
        world = world.evolve(&observer, &evaluator);
        if let Some(
            champ @ Genome {
                fitness: Some(0), ..
            },
        ) = world.best
        {
            println!("\n***** Success! *****");
            return Some(champ);
        };
    }
}

mod evaluation {
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::thread::{JoinHandle, spawn};

    use super::*;

    pub struct Evaluator {
        handle: JoinHandle<()>,
        tx: Sender<Genome>,
        rx: Receiver<Genome>,
    }

    impl Evaluator {
        pub fn spawn(params: &Config) -> Self {
            let (tx, our_rx): (Sender<Genome>, Receiver<Genome>) = channel();
            let (our_tx, rx): (Sender<Genome>, Receiver<Genome>) = channel();

            let target = params.target.clone();

            let handle = spawn(move || {
                for mut genome in our_rx {
                    if genome.fitness.is_none() {
                        genome.fitness = Some(distance::levenshtein(&genome.genes, &target))
                    }
                    our_tx.send(genome).expect("Channel failure");
                }
            });

            Self { handle, tx, rx }
        }

        pub fn evaluate(&self, genome: Genome) -> Genome {
            self.tx.send(genome).expect("tx failure");
            self.rx.recv().expect("rx failure")
        }
    }
}

mod observation {
    use std::sync::mpsc::{channel, Sender, SendError};
    use std::thread::{JoinHandle, spawn};

    use crate::observer::Observe;

    use super::*;

    pub struct Observer {
        pub handle: JoinHandle<()>,
        pub tx: Sender<Genome>,
    }

    struct Window {
        pub frame: Vec<Option<Genome>>,
        i: usize,
        window_size: usize,
    }

    impl Window {
        fn insert(&mut self, thing: Genome) {
            self.i = (self.i + 1) % self.window_size;
            self.frame[self.i] = Some(thing);
            if self.i == 0 {
                self.report();
            }
        }

        fn report(&self) {
            let fitnesses: Vec<Fitness> = self
                .frame
                .iter()
                .filter_map(|t| t.as_ref().and_then(|x| x.fitness))
                .collect();
            let avg_fit = fitnesses.iter().sum::<usize>() as f32 / fitnesses.len() as f32;
            log::info!("Average fitness: {}", avg_fit);
        }

        fn new(window_size: usize) -> Self {
            assert!(window_size > 0);
            Self {
                frame: vec![None; window_size],
                i: 0,
                window_size,
            }
        }
    }

    impl Observe for Observer {
        type Observable = Genome;
        type Params = Config;
        type Error = SendError<Self::Observable>;

        fn spawn(params: &Self::Params) -> Self {
            let (tx, rx) = channel();

            let window_size = params.observer.window_size;
            let handle = spawn(move || {
                let mut window: Window = Window::new(window_size);

                for observable in rx {
                    log::debug!("received observable {:?}", observable);
                    window.insert(observable);
                }
            });

            Self { handle, tx }
        }

        fn observe(&self, ob: Self::Observable) -> Result<(), Self::Error> {
            self.tx.send(ob)
        }

        fn report() {
            unimplemented!()
        }
    }
}
