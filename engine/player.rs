use std::{array, io::{stdout, Write}, f32::consts::PI};

use crate::engine::module::{Effect, Sample};

use super::module::{Module, LoopType, Note, VolEffect};
use sdl2::audio::AudioCallback;

#[derive(Default,Debug,Clone,Copy,clap::ValueEnum)]
pub enum Interpolation {
    #[default]
    None,
    Linear,
    Sinc16,
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

    let quality: i32 = 16;

    for i in 1-quality..quality+1 {
        tmp += vec[((ix+i as f32+vec.len() as f32) % vec.len() as f32) as usize] as f32 * sinc(i as f32-fx)
    };

    tmp
}

impl Channel<'_> {
    fn porta_up(&mut self, mut value: u8) {
        if value != 0 {
            self.porta_memory = value;
        } else {
            value = self.porta_memory;
        }

        //TODO amiga slide

        match value & 0xF0 { // Check if (extra) fine
            0xE0 => self.freq = self.freq * 2f32.powf((value & 0x0F) as f32/768.0), // Extra fine
            0xF0 => self.freq = self.freq * 2f32.powf(2.0*(value & 0x0F) as f32/768.0), // Fine
            _ => self.freq = self.freq * 2f32.powf(4.0*value as f32/768.0), // Regular
        }
    }

    fn porta_down(&mut self, mut value: u8) {
        if value != 0 {
            self.porta_memory = value;
        } else {
            value = self.porta_memory;
        }

        //TODO amiga slide

        match value & 0xF0 { // Check if (extra) fine
            0xE0 => self.freq = self.freq * 2f32.powf(-((value & 0x0F) as f32)/768.0), // Extra fine
            0xF0 => self.freq = self.freq * 2f32.powf(-2.0*(value & 0x0F) as f32/768.0), // Fine
            _ => self.freq = self.freq * 2f32.powf(-4.0*value as f32/768.0), // Regular
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
            // TODO amiga slides
            // Substituting linear slides instead
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

    fn add_to_slab(&mut self, slab: &mut [i32], samplerate: u32, interpolation: Interpolation) {
        let sample = &self.module.samples[self.current_sample_index as usize];

        // Subdivide this tick-slab into 'segments': each time the sample
        // either loops back around or switches direction, we finish the
        // current 'segment' and start a new one.
        
        let mut remaining: u32 = slab.len() as u32;
        let mut pos: u32 = 0;

        while remaining > 0 && self.playing {
            // Figure out how long this segment is, and discount it from
            // remaining.
            let mut seg_ahead = if self.backwards {
                self.position - sample.loop_start as f32
            } else if sample.loop_end > 0 {
                sample.loop_end as f32 - self.position
            } else {
                sample.audio.len() as f32 - self.position
            };

            let mut seg_samples = (seg_ahead * samplerate as f32 / self.freq as f32) as u32;

            // Dirty hack to prevent infinite loops.
            // FIXME: Find a more elegant solution!
            if seg_samples == 0 {
                seg_samples = 1;
            }

            // Make sure we don't write past the slab's end!
            if seg_samples > remaining {
                seg_samples = remaining;
                seg_ahead = seg_samples as f32 * self.freq / samplerate as f32;
            }

            remaining -= seg_samples;

            // Process this segment.
            self.process_segment(sample, seg_samples, seg_ahead, &mut slab[pos as usize..(pos + seg_samples) as usize], samplerate, interpolation);
            pos += seg_samples;
        }
    }

    fn process_segment(&mut self, sample: &Sample, seg_samples: u32, seg_ahead: f32, slab_slice: &mut [i32], samplerate: u32, interpolation: Interpolation) {
        // Make a buffer to store the result of the interpolation of the involved samples.
        let mut interpolated: Vec<i32> = vec![0i32; seg_samples as usize];

        let start = self.position as f32;
        let freq = self.freq / samplerate as f32;

        // NOTE: There is probably a better way to write this, than mostly the
        // same thing but with subtraction on one side and addition on the
        // other. FIXME: do that lol. 
        if self.backwards {
            for (i, val) in interpolated.iter_mut().enumerate() {
                *val = self.interpolation(sample, interpolation, start - (i as f32 * freq));
            }
        }

        else {
            for (i, val) in interpolated.iter_mut().enumerate() {
                //println!("{}", i as f32 * self.freq);
                *val = self.interpolation(sample, interpolation, start + (i as f32 * freq));
            }
        }
        
        // Apply the interpolated buffer to slab_slice.
        for (ival, oval) in interpolated.iter().zip(slab_slice.iter_mut()) {
            *oval = oval.saturating_add(*ival);
        }

        // Advance the position a handful.
        self.advance_position(seg_ahead);
    }

    fn advance_position(&mut self, mut amount: f32) -> bool {
        let sample = &self.module.samples[self.current_sample_index as usize];
        if !self.playing || sample.audio.len() == 0 { return false; };

        while amount > 0.0 {
            let new_position = self.position + if self.backwards {
                -(amount as f32)
            } else {
                amount as f32
            };

            if self.backwards {
                if (new_position as u32) <= sample.loop_start {
                    let offs = sample.loop_start as f32 - new_position;
                    amount -= offs;

                    self.position = sample.loop_start as f32;
                    self.backwards = false;
                }
                

                else {
                    self.position = new_position;
                    amount = 0.0;
                }
            }

            else {
                let real_end = match sample.loop_type {
                    LoopType::None => sample.audio.len() as f32,
                    _ => match sample.loop_end {
                        0 => sample.audio.len() as f32,
                        _ => sample.loop_end as f32,
                    },
                };

                if new_position >= real_end {
                    let offs = real_end - new_position;
                    amount -= offs;

                    match sample.loop_type {
                        LoopType::PingPong => {
                            self.position = real_end;
                            self.backwards = true;
                        },

                        LoopType::Forward => {
                            self.position = sample.loop_start as f32;
                        },

                        _ => {
                            // Stop playing if at the sample end.
                            if new_position as usize >= sample.audio.len() {
                                amount = 0.0;
                                self.playing = false;
                                self.backwards = false;
                            }
                        }
                    }
                }

                else {
                    self.position = new_position;
                    amount = 0.0;
                }
            }
        };

        if !self.playing || sample.audio.len() == 0 { return false };

        true
    }

    fn interpolation(&self, sample: &Sample, interpolation: Interpolation, at: f32) -> i32 {
        match interpolation {
            Interpolation::None => (((sample.audio[at as usize]) as i32*32768) as f32*(self.volume as f32/64.0)*(sample.global_volume as f32/64.0)) as i32,
            Interpolation::Linear => ( ((vec_linear(&sample.audio, at - 1.0)) as i32*32768) as f32*(self.volume/64.0)*(sample.global_volume as f32/64.0)) as i32,
            Interpolation::Sinc16 => ( ((vec_sinc(&sample.audio, at)) as i32*32768) as f32*(self.volume as f32/64.0)*(sample.global_volume as f32/64.0)) as i32
        }
    }

    /*
    fn process(&mut self, samplerate: u32, interpolation: Interpolation) -> i32 {
        let sample = &self.module.samples[self.current_sample_index as usize];
        
        if !self.advance_position(self.freq / samplerate as f32) { return 0; }

        // FIXME detuning in uttitle.it
        self.interpolation(sample, interpolation, self.position)
    }
    */
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
    tick_slab: u32,

    channels: [Channel<'a>;64],
}

impl Player<'_> {
    pub fn from_module(module: &Module, samplerate: u32) -> Player<'_> {
        let mut player = Player {
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
            tick_slab: 0,

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
        };

        player.tick_slab = player.compute_tick_slab();

        player
    }

    pub fn process_to_buffer(&mut self, buf: &mut [i32]) {
        let num_samples = buf.len();
        let total_counter = num_samples + self.tick_counter as usize;
        let num_ticks = (total_counter as f64 / self.tick_slab as f64).floor() as usize;
        let extra_counter = total_counter % self.tick_slab as usize;

        let mut this_pos: usize = 0;
        let mut next_pos = self.tick_slab as usize - self.tick_counter as usize;

        buf.fill(0);

        // Mix and process each tick
        for i in 0..num_ticks {
            for c in self.channels.iter_mut() {
                c.add_to_slab(&mut buf[this_pos..next_pos], self.samplerate, self.interpolation);
            }

            this_pos = next_pos;
            next_pos = this_pos + self.tick_slab as usize;

            self.ticks_passed += 1;

            if self.ticks_passed >= self.current_speed {
                self.advance_row();
                self.play_row();
            }

            self.process_tick();
        }

        // Mix any remaining audio
        if this_pos < buf.len() {
            for c in self.channels.iter_mut() {
                c.add_to_slab(&mut buf[this_pos..], self.samplerate, self.interpolation);
            }
        }

        self.tick_counter = extra_counter as u32;
    }

    /*
    pub fn process(&mut self) -> i32 {
        let mut out = 0i32;

        for c in self.channels.iter_mut() {
            out = out.saturating_add(c.process(self.samplerate, self.interpolation));
        };

        if self.tick_counter >= self.tick_slab {
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
    */

    fn process_tick(&mut self) {
        if self.current_row == 65535 { return };
        let row = &self.module.patterns[self.current_pattern as usize][self.current_row as usize];

        for (i,col) in row.iter().enumerate() {
            let channel = &mut self.channels[i];

            match col.effect {
                Effect::PortaUp(value) => channel.porta_up(value),
                Effect::PortaDown(value) => channel.porta_down(value),
                Effect::TonePorta(value) => channel.tone_portamento(col.note, self.module.linear_freq_slides, value),
                Effect::VolSlide(value) => channel.vol_slide(value),
                Effect::Retrig(value) => channel.retrigger(value),
                _ => {}
            }
        }
    }

    fn set_tempo(&mut self, tempo: u8) {
        self.current_tempo = tempo;
        self.tick_slab = self.compute_tick_slab(); 
    }

    fn compute_tick_slab(&self) -> u32 {
        ((self.samplerate as f32*2.5)/self.current_tempo as f32) as u32
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
                Effect::SetTempo(tempo) => self.set_tempo(tempo),
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
        self.process_to_buffer(out);
    }
}
