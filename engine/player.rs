use std::{array, io::{stdout, Write}, f32::consts::PI};

use crate::engine::module::Effect;

use super::module::{Module, LoopType, Note, VolEffect};
use sdl2::audio::AudioCallback;

#[derive(Default,Debug,Clone,Copy,clap::ValueEnum)]
pub enum Interpolation {
    #[default]
    None,
    Linear,
    Sinc8,
    // Sinc32,
    // Sinc64
}

#[derive(Clone)]
struct Channel<'a> {
    module: &'a Module,

    current_sample_index: u8,
    playing: bool,
    freq: f32,
    position: f32,
    backwards: bool,

    porta_memory: u8, // Exx, Fxx, Gxx
    last_note: u8, // Gxx
    offset_memory: u8, // Oxx
    volume_memory: u8, // Dxy
    retrigger_ticks: u8, // Qxy

    volume: f32,
    // panning: i8,
}

fn sinc(x: f32) -> f32 {
    if x <= 0.0001 && x >= -0.0001 {return 1.0};
    (x*PI).sin()/(x*PI)
}

fn vec_linear(vec: &Vec<i16>, index: f32) -> i16 {
    (vec[index.floor() as usize] as f32+(index-index.floor())*((vec[index.ceil() as usize] as f32-vec[index.floor() as usize] as f32)*(index-index.floor()))) as i16
}

fn vec_sinc(vec: &Vec<i16>, index: f32) -> f32 {
    let ix = index.floor();
    let fx = index - ix;
    let mut tmp = 0f32;

    let quality: i32 = 8;

    for i in 1-quality..quality+1 {
        tmp += vec[((ix+i as f32+vec.len() as f32) % vec.len() as f32) as usize] as f32 * sinc(i as f32-fx)
    };

    tmp
}

impl Channel<'_> {
    fn porta_up(&mut self, linear: bool, mut value: u8) {
        if value != 0 {
            self.porta_memory = value;
        } else {
            value = self.porta_memory;
        }

        if linear {
            match value & 0xF0 { // Detect fine-iness
                0xE0 => self.freq = self.freq * 2f32.powf((value & 0x0F) as f32/768.0), // Extra fine
                0xF0 => self.freq = self.freq * 2f32.powf(2.0*(value & 0x0F) as f32/768.0), // Fine
                _ => self.freq = self.freq * 2f32.powf(4.0*value as f32/768.0), // Regular
            }
        } else { // Amiga slide
            // FIXME this is a rough approximation without using periods, LUTs or any of that shit
            match value & 0xF0 {
                0xE0 => self.freq += (self.freq/8363.0)*8.0*value as f32,
                0xF0 => self.freq += (self.freq/8363.0)*16.0*value as f32,
                _ => self.freq += (self.freq/8363.0)*32.0*value as f32
            }
        }
    }

    fn porta_down(&mut self, linear: bool, mut value: u8) {
        if value != 0 {
            self.porta_memory = value;
        } else {
            value = self.porta_memory;
        }

        if linear {
            match value & 0xF0 { // Detect fine-iness
                0xE0 => self.freq = self.freq * 2f32.powf(-((value & 0x0F) as f32)/768.0), // Extra fine
                0xF0 => self.freq = self.freq * 2f32.powf(-2.0*(value & 0x0F) as f32/768.0), // Fine
                _ => self.freq = self.freq * 2f32.powf(-4.0*value as f32/768.0), // Regular
            }
        } else { // Amiga slide
            // FIXME this is a rough approximation without using periods, LUTs or any of that shit
            match value & 0xF0 {
                0xE0 => self.freq -= (self.freq/8363.0)*8.0*value as f32,
                0xF0 => self.freq -= (self.freq/8363.0)*16.0*value as f32,
                _ => self.freq -= (self.freq/8363.0)*32.0*value as f32
            }
        }
    }

    fn tone_portamento(&mut self, note: Note, linear: bool, mut value: u8) {
        if value != 0 {
            self.porta_memory = value;
        } else {
            value = self.porta_memory;
        }

        match note {
            Note::On(key) => self.last_note = key,
            _ => {}
        }

        let desired_freq = 2f32.powf((self.last_note as f32-60.0)/12.0)*self.module.samples[self.current_sample_index as usize].base_frequency as f32;

        if linear {
            if self.freq < desired_freq {
                self.freq = self.freq * 2f32.powf(4.0*value as f32/768.0);
                if self.freq > desired_freq {
                    self.freq = desired_freq
                }
            } else if self.freq > desired_freq {
                self.freq = self.freq * 2f32.powf(-4.0*value as f32/768.0);
                if self.freq < desired_freq {
                    self.freq = desired_freq
                }
            }
        } else { // Amiga slides
            // FIXME this is a rough approximation without using periods, LUTs or any of that shit
            if self.freq < desired_freq {
                self.freq += (self.freq/8363.0)*32.0*value as f32;
                if self.freq > desired_freq {
                    self.freq = desired_freq
                }
            } else if self.freq > desired_freq {
                self.freq -= (self.freq/8363.0)*32.0*value as f32;
                if self.freq < desired_freq {
                    self.freq = desired_freq
                }
            }
        }
    }

    fn vol_slide(&mut self, mut value: u8) {
        if value != 0 {
            self.volume_memory = value;
        } else {
            value = self.volume_memory;
        }

        let upper = (value & 0xF0) >> 4;
        let lower = value & 0x0F;

        // FIXME accuracy?
        if upper == 0 && lower != 0 { // regular down
            self.volume -= lower as f32;
        } else if upper != 0 && lower == 0 { // regular up
            self.volume += upper as f32;

        // FIXME reimplement fine volumes, this sucks
        } else if upper == 0xF && lower != 0 { // fine down
            self.volume -= lower as f32/8.0;
        } else if upper != 0 && lower == 0xF { // fine up
            self.volume += upper as f32/8.0;
        } else if upper != 0 && lower != 0 {
            println!("Channel::vol_slide: invalid argument {:X}, ignoring", value);
        }

        if self.volume > 64.0 { self.volume = 64.0 };
        if self.volume < 0.0 { self.volume = 0.0 }
    }

    fn retrigger(&mut self, value: u8) {
        match (value & 0xF0) >> 4 { // Volume change
            // TODO last used value for XM
            //0 => {}
            1 => self.volume -= 1.0,
            2 => self.volume -= 2.0,
            3 => self.volume -= 4.0,
            4 => self.volume -= 8.0,
            5 => self.volume -= 16.0,
            6 => self.volume *= 2.0/3.0,
            7 => self.volume *= 0.5,

            9 => self.volume += 1.0,
            0xA => self.volume += 2.0,
            0xB => self.volume += 4.0,
            0xC => self.volume += 8.0,
            0xD => self.volume += 16.0,
            0xE => self.volume *= 1.5,
            0xF => self.volume *= 2.0,

            _ => {}
        }

        if self.retrigger_ticks >= value & 0x0F { 
            self.position = 0.0;
            self.retrigger_ticks = 0;
        };

        self.retrigger_ticks += 1;

        if self.volume > 64.0 { self.volume = 64.0 };
        if self.volume < 0.0 { self.volume = 0.0 }
    }

    fn process(&mut self, samplerate: u32, interpolation: Interpolation) -> i32 {
        let sample = &self.module.samples[self.current_sample_index as usize];
        if !self.playing || sample.audio.len() == 0 { return 0 };

        if self.backwards {
            if self.position as u32 <= sample.loop_start {
                self.backwards = false
            } else {
                self.position -= self.freq/samplerate as f32;
            }
        } else {
            self.position += self.freq/samplerate as f32;
        }

        if sample.loop_end > 0 {
            if self.position as u32 > sample.loop_end-1 {
                match sample.loop_type {
                    LoopType::Forward => self.position = sample.loop_start as f32,
                    LoopType::PingPong => { self.backwards = true; self.position -= self.freq/samplerate as f32; }, // self.position -= 1.0 or 2.0 does not work as the program errors with out of bounds
                    _ => {},
                }
            }
        }

        // Prevent out of bounds, but it doesn't seem to be working reliably
        if self.position as usize >= sample.audio.len()-1 && matches!(sample.loop_type, LoopType::None) {
            self.playing = false; self.backwards = false;
        }

        if !self.playing { return 0 };

        // FIXME detuning in uttitle.it
        match interpolation {
            Interpolation::None => (((sample.audio[self.position as usize]) as i32*32768) as f32*(self.volume as f32/64.0)*(sample.global_volume as f32/64.0)) as i32,
            Interpolation::Linear => ( ((vec_linear(&sample.audio, self.position-1.0)) as i32*32768) as f32*(self.volume/64.0)*(sample.global_volume as f32/64.0)) as i32,
            Interpolation::Sinc8 => ( ((vec_sinc(&sample.audio, self.position)) as i32*32768) as f32*(self.volume as f32/64.0)*(sample.global_volume as f32/64.0)) as i32
        }
    }
}

pub struct Player<'a> {
    pub module: &'a Module,

    pub samplerate: u32,
    pub interpolation: Interpolation,

    current_position: u8,
    current_pattern: u8,
    current_row: u16,

    current_tempo: u8,
    current_speed: u8,

    tick_counter: u32,
    ticks_passed: u8,

    channels: [Channel<'a>;64],
}

impl Player<'_> {
    pub fn from_module(module: &Module, samplerate: u32) -> Player<'_> {
        Player {
            module,

            samplerate,
            interpolation: Interpolation::Linear,

            current_position: 0,
            current_pattern: module.playlist[0],
            current_row: 65535,

            current_tempo: module.initial_tempo,
            current_speed: module.initial_speed,

            tick_counter: 0,
            ticks_passed: 0,

            channels: array::from_fn(|_| Channel {
                module: module,

                current_sample_index: 0,
                playing: false,
                freq: 8363.0,
                position: 0.0,
                backwards: false,

                porta_memory: 0,
                last_note: 0,
                offset_memory: 0,
                volume_memory: 0,
                retrigger_ticks: 0,

                volume: 64.0,
                // panning: 0
            }),
        }
    }

    pub fn process(&mut self) -> i32 {
        let mut out = 0i32;

        for c in self.channels.iter_mut() {
            out = out.saturating_add(c.process(self.samplerate, self.interpolation));
        };

        if self.tick_counter >= ((self.samplerate as f32*2.5)/self.current_tempo as f32) as u32 {
            self.ticks_passed += 1;
            self.tick_counter = 0;
            if self.ticks_passed >= self.current_speed {
                self.advance_row();
                self.play_row();
            }
            self.process_tick();
        } else {
            self.tick_counter += 1;
        }

        out
    }

    fn process_tick(&mut self) {
        if self.current_row == 65535 { return };
        let row = &self.module.patterns[self.current_pattern as usize][self.current_row as usize];

        for (i,col) in row.iter().enumerate() {
            let channel = &mut self.channels[i];

            match col.effect {
                Effect::PortaUp(value) => channel.porta_up(self.module.linear_freq_slides, value),
                Effect::PortaDown(value) => channel.porta_down(self.module.linear_freq_slides, value),
                Effect::TonePorta(value) => channel.tone_portamento(col.note, self.module.linear_freq_slides, value),
                Effect::VolSlide(value) => channel.vol_slide(value),
                Effect::Retrig(value) => channel.retrigger(value),
                _ => {}
            }
        }
    }

    fn advance_row(&mut self) {
        if self.current_row == 65535 { self.current_row = 0; self.ticks_passed = 0; return; };

        let row = &self.module.patterns[self.current_pattern as usize][self.current_row as usize];
        let mut pos_jump_enabled = false;
        let mut pos_jump_to = 0u8;

        let mut pat_break_enabled = false;
        let mut pat_break_to = 0u8;

        for col in row.iter() {
            match col.effect {
                Effect::SetSpeed(speed) => self.current_speed = speed,
                Effect::SetTempo(tempo) => self.current_tempo = tempo,
                Effect::PosJump(position) => { pos_jump_enabled = true; pos_jump_to = position },
                Effect::PatBreak(row) => { pat_break_enabled = true; pat_break_to = row },
                _ => {}
            }
        }

        self.ticks_passed = 0;
        if self.current_row == self.module.patterns[self.current_pattern as usize].len() as u16 {
            self.current_row = 0;
        } else {
            self.current_row += 1;
            if pos_jump_enabled {
                self.current_row = 0;
                self.current_position = pos_jump_to;
                self.current_pattern = self.module.playlist[self.current_position as usize];
            }

            if pat_break_enabled {
                self.current_row = pat_break_to as u16;
                self.current_position += 1;
                self.current_pattern = self.module.playlist[self.current_position as usize];

                if self.current_pattern == 255 {
                    self.current_position = 0;
                    self.current_pattern = self.module.playlist[self.current_position as usize];
                }
            }
        }

        if self.current_row as usize == self.module.patterns[self.current_pattern as usize].len() {
            self.current_row = 0;
            self.current_position += 1;
            self.current_pattern = self.module.playlist[self.current_position as usize];

            if self.current_pattern == 255 { // End of song marker
                std::process::exit(0);
            }
        };
    }

    fn play_row(&mut self) {
        let row = &self.module.patterns[self.current_pattern as usize][self.current_row as usize];

        print!("Position {}, Pattern {}, Row {}\x1b[K\r", self.current_position, self.current_pattern, self.current_row);
        stdout().flush().unwrap();

        for (i,col) in row.iter().enumerate() {
            let channel = &mut self.channels[i];

            /* match col.effect {
                _ => {}
                //TODO effects
            } */

            match col.vol {
                // TODO volume commands
                VolEffect::None => {},
                VolEffect::FineVolSlideUp(_) => {},
                VolEffect::FineVolSlideDown(_) => {},
                VolEffect::VolSlideUp(_) => {},
                VolEffect::VolSlideDown(_) => {},
                VolEffect::PortaDown(_) => {},
                VolEffect::PortaUp(_) => {},
                VolEffect::TonePorta(_) => {},
                VolEffect::VibratoDepth(_) => {},
                VolEffect::SetPan(_) => {},
                VolEffect::Volume(volume) => channel.volume = volume as f32,
            }

            if col.instrument != 0 {
                channel.current_sample_index = col.instrument-1;

                if matches!(col.vol, VolEffect::None) {
                    channel.volume = self.module.samples[channel.current_sample_index as usize].default_volume as f32
                }
            }

            match col.note {
                Note::None => {},
                Note::On(note) => {
                    if !matches!(col.effect, Effect::TonePorta(_)) && !matches!(col.vol, VolEffect::TonePorta(_)) {
                        channel.playing = true;
                        channel.position = match col.effect {
                            Effect::SampleOffset(position) => { if position != 0 { channel.offset_memory = position }; channel.offset_memory as f32 * 256.0 },
                            _ => 0.0
                        };
                        channel.freq = 2f32.powf((note as f32-60.0)/12.0)*self.module.samples[channel.current_sample_index as usize].base_frequency as f32;
                    }
                },
                Note::Fade => {},
                Note::Cut => channel.playing = false,
                Note::Off => channel.playing = false,
            }
        }
    }
}

impl AudioCallback for Player<'_> {
    type Channel = i32;

    fn callback(&mut self, out: &mut [i32]) {
        for s in out.iter_mut() {
            *s = self.process();
        }
    }
}