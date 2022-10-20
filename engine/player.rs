use std::{
    array,
    f32::consts::PI,
    io::{stdout, Write},
};

use crate::engine::module::{Effect, Sample};

use super::module::{LoopType, Module, Note, VolEffect};
use sdl2::audio::AudioCallback;

#[derive(Default, Debug, Clone, Copy, clap::ValueEnum)]
pub enum Interpolation {
    #[default]
    None,
    Linear,
    Sinc8,
    Sinc16,
    Sinc32,
    // Sinc64
}

#[derive(Clone)]
struct Channel<'a> {
    module: &'a Module,

    current_sample_index: u8,
    playing: bool,
    freq: f32,
    position: f64,
    backwards: bool,

    porta_memory: u8,    // Exx, Fxx, Gxx
    last_note: u8,       // Gxx
    offset_memory: u8,   // Oxx
    volume_memory: u8,   // Dxy
    retrigger_ticks: u8, // Qxy

    volume: f32,
    // panning: i8,
}

fn sinc(x: f32) -> f32 {
    if x <= 0.0001 && x >= -0.0001 {
        return 1.0;
    };
    (x * PI).sin() / (x * PI)
}

// Interpolation functions that operate on buffers.
fn buf_linear(from: &[i16], to: &mut [i32], backwards: bool) {
    if from.len() == 1 {
        // Special case handling
        to.fill(from[0] as i32);
        return;
    }

    let ratio = (from.len() - 1) as f32 / (to.len() - 1) as f32;
    let flen = from.len() as f32;

    for (i, res) in to.iter_mut().enumerate() {
        let x = i as f32 * ratio;
        let x = if backwards { flen - x - 1.0 } else { x };
        let x = (x - 0.0001).max(0.0); /* ugly hack to prevent ix + 1 OOB */
        let ix = x.floor() as usize;
        let alpha = x - x.floor();

        *res = ((from[ix] as f32 * (1.0 - alpha) + from[ix + 1] as f32 * alpha) * 32768.0) as i32;
    }
}

fn buf_sinc(from: &[i16], to: &mut [i32], backwards: bool, quality: isize, pingpong: bool) {
    let ratio = (from.len() - 1) as f32 / to.len() as f32;
    let blen = from.len() as isize;
    let flen = blen as f32;
    let mut tmp = vec![0.0f32; to.len() as usize];

    for iter in (1isize - quality)..(quality + 1isize) {
        for (i, res) in tmp.iter_mut().enumerate() {
            let x = i as f32 * ratio;
            let x = if backwards { flen - x - 1.0 } else { x };
            let cx = x.floor();
            let ix = cx as isize + iter;
            let fx = x - cx;

            let ix = if pingpong {
                if ix < 0 {
                    (-ix).min(blen - 1)
                } else if ix >= blen {
                    (2 * blen - ix - 2).max(0)
                } else {
                    ix
                }
            } else {
                ((ix % blen) + blen) % blen
            };

            *res += from[ix as usize] as f32 * sinc(iter as f32 - fx);
        }
    }

    for (tn, res) in tmp.iter().zip(to.iter_mut()) {
        *res = (*tn * 32768.0) as i32;
    }
}

fn period(freq: f32) -> f32 {
    3546816.0 / freq
}

fn freq_from_period(period: u16) -> f32 {
    3546816.0 / period as f32
}

impl Channel<'_> {
    pub fn new(module: &'_ Module) -> Channel<'_> {
        Channel {
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
        }
    }

    fn porta_up(&mut self, linear: bool, mut value: u8) {
        if value != 0 {
            self.porta_memory = value;
        } else {
            value = self.porta_memory;
        }

        if linear {
            match value & 0xF0 {
                // Detect fine-iness
                0xE0 => self.freq = self.freq * 2f32.powf((value & 0x0F) as f32 / 768.0), // Extra fine
                0xF0 => self.freq = self.freq * 2f32.powf(2.0 * (value & 0x0F) as f32 / 768.0), // Fine
                _ => self.freq = self.freq * 2f32.powf(4.0 * value as f32 / 768.0), // Regular
            }
        } else {
            // Amiga slide
            // TODO fine slides
            match value & 0xF0 {
                0xE0 => self.freq += (self.freq / 8363.0) * 8.0 * value as f32,
                0xF0 => self.freq += (self.freq / 8363.0) * 16.0 * value as f32,
                _ => self.freq = freq_from_period((period(self.freq) - (value as f32)) as u16),
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
            match value & 0xF0 {
                // Detect fine-iness
                0xE0 => self.freq = self.freq * 2f32.powf(-((value & 0x0F) as f32) / 768.0), // Extra fine
                0xF0 => self.freq = self.freq * 2f32.powf(-2.0 * (value & 0x0F) as f32 / 768.0), // Fine
                _ => self.freq = self.freq * 2f32.powf(-4.0 * value as f32 / 768.0), // Regular
            }
        } else {
            // Amiga slide
            // TODO fine slides
            match value & 0xF0 {
                0xE0 => self.freq -= (self.freq / 8363.0) * 8.0 * value as f32,
                0xF0 => self.freq -= (self.freq / 8363.0) * 16.0 * value as f32,
                _ => self.freq = freq_from_period((period(self.freq) + (value as f32)) as u16),
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

        let desired_freq = 2f32.powf((self.last_note as f32 - 60.0) / 12.0)
            * self.module.samples[self.current_sample_index as usize].base_frequency as f32;

        if linear {
            if self.freq < desired_freq {
                self.freq = self.freq * 2f32.powf(4.0 * value as f32 / 768.0);
                if self.freq > desired_freq {
                    self.freq = desired_freq
                }
            } else if self.freq > desired_freq {
                self.freq = self.freq * 2f32.powf(-4.0 * value as f32 / 768.0);
                if self.freq < desired_freq {
                    self.freq = desired_freq
                }
            }
        } else {
            // Amiga slides
            if self.freq < desired_freq {
                self.freq = freq_from_period((period(self.freq) - (value as f32)) as u16);
                if self.freq > desired_freq {
                    self.freq = desired_freq
                }
            } else if self.freq > desired_freq {
                self.freq = freq_from_period((period(self.freq) + (value as f32)) as u16);
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
        if upper == 0 && lower != 0 {
            // regular down
            self.volume -= lower as f32;
        } else if upper != 0 && lower == 0 {
            // regular up
            self.volume += upper as f32;

        // FIXME reimplement fine volumes, this sucks
        } else if upper == 0xF && lower != 0 {
            // fine down
            self.volume -= lower as f32 / 8.0;
        } else if upper != 0 && lower == 0xF {
            // fine up
            self.volume += upper as f32 / 8.0;
        } else if upper != 0 && lower != 0 {
            println!("Channel::vol_slide: invalid argument {:X}, ignoring", value);
        }

        if self.volume > 64.0 {
            self.volume = 64.0
        };
        if self.volume < 0.0 {
            self.volume = 0.0
        }
    }

    fn retrigger(&mut self, value: u8) {
        match (value & 0xF0) >> 4 {
            // Volume change
            // TODO last used value for XM
            //0 => {}
            1 => self.volume -= 1.0,
            2 => self.volume -= 2.0,
            3 => self.volume -= 4.0,
            4 => self.volume -= 8.0,
            5 => self.volume -= 16.0,
            6 => self.volume *= 2.0 / 3.0,
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

        if self.volume > 64.0 {
            self.volume = 64.0
        };
        if self.volume < 0.0 {
            self.volume = 0.0
        }
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
                self.position - sample.loop_start as f64
            } else if sample.loop_end > 0 {
                sample.loop_end as f64 - self.position
            } else {
                sample.audio.len() as f64 - self.position
            };

            let mut seg_samples = (seg_ahead * samplerate as f64 / self.freq as f64) as u32;

            // Dirty hack to prevent infinite loops.
            // FIXME: Find a more elegant solution!
            if seg_samples == 0 {
                seg_samples = 1;
            }

            // Make sure we don't write past the slab's end!
            if seg_samples > remaining {
                seg_samples = remaining;
                seg_ahead = seg_samples as f64 * self.freq as f64 / samplerate as f64;
            }

            remaining -= seg_samples;

            // Sanity check - skip if segment is tiny.
            if seg_samples == 0 || seg_ahead < 1.0 {
                pos += seg_samples.max(1);
                continue;
            }

            // Process this segment's audio.
            self.process_segment(
                sample,
                seg_samples,
                seg_ahead,
                &mut slab[pos as usize..(pos + seg_samples) as usize],
                samplerate,
                interpolation,
            );

            // Advance position.
            pos += seg_samples;
        }
    }

    fn process_segment(
        &mut self,
        sample: &Sample,
        seg_samples: u32,
        seg_ahead: f64,
        slab_slice: &mut [i32],
        samplerate: u32,
        interpolation: Interpolation,
    ) {
        // Make a buffer to store the result of the interpolation of the
        // involved samples.
        let mut interpolated: Vec<i32> = vec![0i32; seg_samples as usize];
        let freq = self.freq as f64 / samplerate as f64;

        // Find the correct indices for the slice of sample audio.
        let pos_a = self.position as usize;
        let pos_b = (self.position
            + seg_samples as f64 * freq * if self.backwards { -1.0 } else { 1.0 })
            as usize;

        let pos_1 = if self.backwards { pos_b } else { pos_a };
        let pos_2 = if self.backwards { pos_a } else { pos_b };

        // Interpolate relevant sample audio;
        self.interpolate_buffers(
            &sample.audio[pos_1..pos_2],
            &mut interpolated,
            interpolation,
        );

        // Apply volumes to interpolated audio.
        self.apply_volumes(&mut interpolated);

        // Apply the interpolated buffer to slab_slice.
        for (ival, oval) in interpolated.iter().zip(slab_slice.iter_mut()) {
            *oval = oval.saturating_add(*ival);
        }

        // Advance the position a handful.
        self.advance_position(seg_ahead);
    }

    fn advance_position(&mut self, mut amount: f64) -> bool {
        let sample = &self.module.samples[self.current_sample_index as usize];
        if !self.playing || sample.audio.len() == 0 {
            return false;
        };

        while amount > 0.0 {
            let new_position = self.position
                + if self.backwards {
                    -(amount as f64)
                } else {
                    amount as f64
                };

            if self.backwards {
                if (new_position as u32) <= sample.loop_start {
                    let offs = sample.loop_start as f64 - new_position;
                    amount -= offs;

                    self.position = sample.loop_start as f64;
                    self.backwards = false;
                } else {
                    self.position = new_position;
                    amount = 0.0;
                }
            } else {
                let real_end = match sample.loop_type {
                    LoopType::None => sample.audio.len() as f64,
                    _ => match sample.loop_end {
                        0 => sample.audio.len() as f64,
                        _ => sample.loop_end as f64,
                    },
                };

                if new_position >= real_end {
                    let offs = real_end - new_position;
                    amount -= offs;

                    match sample.loop_type {
                        LoopType::PingPong => {
                            self.position = real_end;
                            self.backwards = true;
                        }

                        LoopType::Forward => {
                            self.position = sample.loop_start as f64;
                        }

                        _ => {
                            // Stop playing if at the sample end.
                            if new_position as usize >= sample.audio.len() {
                                amount = 0.0;
                                self.playing = false;
                                self.backwards = false;
                            }
                        }
                    }
                } else {
                    self.position = new_position;
                    amount = 0.0;
                }
            }
        }

        if !self.playing || sample.audio.len() == 0 {
            return false;
        };

        true
    }

    fn interpolate_buffers(&self, from: &[i16], to: &mut [i32], interpolation: Interpolation) {
        let sample = &self.module.samples[self.current_sample_index as usize];
        let pingpong = match sample.loop_type {
            LoopType::PingPong => true,
            _ => false,
        };

        match interpolation {
            Interpolation::None => {
                let ratio = from.len() as f32 / to.len() as f32;
                for (iy, res) in to.iter_mut().enumerate() {
                    let ix = (iy as f32 * ratio) as usize;
                    *res = from[ix] as i32 * 32768;
                }
            }
            Interpolation::Linear => buf_linear(from, to, self.backwards),
            Interpolation::Sinc8 => buf_sinc(from, to, self.backwards, 8, pingpong),
            Interpolation::Sinc16 => buf_sinc(from, to, self.backwards, 16, pingpong),
            Interpolation::Sinc32 => buf_sinc(from, to, self.backwards, 32, pingpong),
        };
    }

    fn apply_volumes(&self, at: &mut [i32]) {
        let sample = &self.module.samples[self.current_sample_index as usize];
        let sample_vol = sample.global_volume as f32 / 64.0;
        let global_vol = self.volume / 64.0;
        let total_vol = sample_vol * global_vol;

        for val in at.iter_mut() {
            *val = (*val as f32 * total_vol) as i32;
        }
    }
}

pub struct Player<'a> {
    pub module: &'a Module,

    pub samplerate: u32,
    pub interpolation: Interpolation,

    pub current_position: u8,
    pub current_pattern: u8,
    current_row: u16,

    current_tempo: u8,
    current_speed: u8,

    tick_counter: u32,
    ticks_passed: u8,
    tick_slab: u32,

    channels: Vec<Channel<'a>>,
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

            channels: Vec::new(),
        };

        player.channels = vec![
            Channel::new(module);
            module
                .patterns
                .iter()
                .map(|x| x.iter().map(|x| x.len()).max().unwrap_or(0))
                .max()
                .unwrap_or(0)
        ];

        player.tick_slab = player.compute_tick_slab();

        player
    }

    pub fn process_to_buffer(&mut self, buf: &mut [i32]) {
        // The size of this buffer window, in samples.
        let num_samples = buf.len();

        // How many samples since the previous tick boundary at the end of this
        // buffer window.
        let total_counter = num_samples + self.tick_counter as usize;

        // How many tick boundaries are within the buffer window.
        let num_ticks = (total_counter as f32 / self.tick_slab as f32).floor() as usize;

        // The final state of self.tick_counter.
        let extra_counter = total_counter % self.tick_slab as usize;

        // The position of a tick boundary within the buffer window.
        let tick_offs = self.tick_slab as usize - self.tick_counter as usize;

        // How many samples to be rendered after the last tick boundary.
        // Not the same as extra_counter - this only takes into account the
        // CURRENT buffer window, not the past ones within the same tick.
        let remaining = extra_counter.min(num_samples);

        // Reset audio buffer.
        buf.fill(0);

        // Mix and process each tick.
        for i in 0i32..num_ticks as i32 {
            let ipos = tick_offs as i32 + (i - 1) * self.tick_slab as i32;

            for c in self.channels.iter_mut() {
                c.add_to_slab(
                    &mut buf[ipos.max(0) as usize..(ipos + self.tick_slab as i32) as usize],
                    self.samplerate,
                    self.interpolation,
                );
            }

            self.ticks_passed += 1;

            if self.ticks_passed >= self.current_speed {
                self.advance_row();
                self.play_row();
            }

            self.process_tick();
        }

        // Mix any remaining audio.
        if remaining > 0 {
            for c in self.channels.iter_mut() {
                c.add_to_slab(
                    &mut buf[num_samples - remaining..],
                    self.samplerate,
                    self.interpolation,
                );
            }
        }

        self.tick_counter = extra_counter as u32;
    }

    fn process_tick(&mut self) {
        if self.current_row == 65535 {
            return;
        };
        let row = &self.module.patterns[self.current_pattern as usize][self.current_row as usize];

        for (i, col) in row.iter().enumerate() {
            let channel = &mut self.channels[i];

            match col.effect {
                Effect::PortaUp(value) => channel.porta_up(self.module.linear_freq_slides, value),
                Effect::PortaDown(value) => {
                    channel.porta_down(self.module.linear_freq_slides, value)
                }
                Effect::TonePorta(value) => {
                    channel.tone_portamento(col.note, self.module.linear_freq_slides, value)
                }
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
        ((self.samplerate as f64 * 2.5) / self.current_tempo as f64) as u32
    }

    fn advance_row(&mut self) {
        if self.current_row == 65535 {
            self.current_row = 0;
            self.ticks_passed = 0;
            return;
        };

        let row = &self.module.patterns[self.current_pattern as usize][self.current_row as usize];
        let mut pos_jump_enabled = false;
        let mut pos_jump_to = 0u8;

        let mut pat_break_enabled = false;
        let mut pat_break_to = 0u8;

        for col in row.iter() {
            match col.effect {
                Effect::SetSpeed(speed) => self.current_speed = speed,
                Effect::SetTempo(tempo) => self.set_tempo(tempo),
                Effect::PosJump(position) => {
                    pos_jump_enabled = true;
                    pos_jump_to = position
                }
                Effect::PatBreak(row) => {
                    pat_break_enabled = true;
                    pat_break_to = row
                }
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

            if self.current_pattern == 255 {
                // End of song marker
                std::process::exit(0);
            }
        };
    }

    fn play_row(&mut self) {
        let row = &self.module.patterns[self.current_pattern as usize][self.current_row as usize];

        for (i, col) in row.iter().enumerate() {
            let channel = &mut self.channels[i];

            /* match col.effect {
                _ => {}
                //TODO effects
            } */

            match col.vol {
                // TODO volume commands
                VolEffect::None => {}
                VolEffect::FineVolSlideUp(_) => {}
                VolEffect::FineVolSlideDown(_) => {}
                VolEffect::VolSlideUp(_) => {}
                VolEffect::VolSlideDown(_) => {}
                VolEffect::PortaDown(_) => {}
                VolEffect::PortaUp(_) => {}
                VolEffect::TonePorta(_) => {}
                VolEffect::VibratoDepth(_) => {}
                VolEffect::SetPan(_) => {}
                VolEffect::Volume(volume) => channel.volume = volume as f32,
            }

            if col.instrument != 0 {
                channel.current_sample_index = col.instrument - 1;

                if matches!(col.vol, VolEffect::None) {
                    channel.volume = self.module.samples[channel.current_sample_index as usize]
                        .default_volume as f32
                }
            }

            match col.note {
                Note::None => {}
                Note::On(note) => {
                    if !matches!(col.effect, Effect::TonePorta(_))
                        && !matches!(col.vol, VolEffect::TonePorta(_))
                    {
                        channel.playing = true;
                        channel.backwards = false;

                        channel.position = match col.effect {
                            Effect::SampleOffset(position) => {
                                if position != 0 {
                                    channel.offset_memory = position
                                };
                                channel.offset_memory as f64 * 256.0
                            }
                            _ => 0.0,
                        };

                        channel.freq = 2f32.powf((note as f32 - 60.0) / 12.0)
                            * self.module.samples[channel.current_sample_index as usize]
                                .base_frequency as f32;
                    }
                }
                Note::Fade => {}
                Note::Cut => channel.playing = false,
                Note::Off => channel.playing = false,
            }
        }

        print!(
            "[Position {}, Pattern {}, Row {}]\x1b[K\n\x1b[K\nChannels:\x1b[K\n",
            self.current_position, self.current_pattern, self.current_row
        );

        for (i, channel) in self.channels.iter().enumerate() {
            if !channel.playing {
                println!(" {:>3} -\x1b[K", i + 1);
            } else {
                println!(
                    " {:>3} : sample {:<4}  volume {:>05.2}   freq {:<6.2}\x1b[K",
                    i + 1,
                    channel.current_sample_index,
                    channel.volume,
                    channel.freq
                );
            }
        }

        print!("\x1b[{}F", self.channels.len() + 3);

        stdout().flush().unwrap();
    }
}

impl AudioCallback for Player<'_> {
    type Channel = i32;

    fn callback(&mut self, out: &mut [i32]) {
        self.process_to_buffer(out);
    }
}
