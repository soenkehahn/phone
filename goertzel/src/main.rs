use std::f64::consts;
use std::sync::mpsc::channel;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SupportedStreamConfig};
use hound;
use pico_args::Arguments;
use ringbuf::storage::Heap;
use ringbuf::SharedRb;
use ringbuf::traits::{RingBuffer, Consumer};


const WINDOW_INTERVAL: u32 = 1000;
const CHUNK_SIZE: u32 = 2000;
const SAMPLE_FREQ: u32 = 48000;
const FREQS: [u32;7] = [697, 770, 852, 941, 1209, 1336, 1477];


fn goertzel_coeff(target_freq: u32, sample_freq: u32) -> f64 {
    2.0 * (
        2.0 * consts::PI / CHUNK_SIZE as f64 * (
            0.5 + CHUNK_SIZE as f64 * target_freq as f64 / sample_freq as f64
        )
    ).cos()
}


struct Goertzeler {
    ariana_goertzde: [(f64, SharedRb::<Heap<f64>>);7]
}

impl Goertzeler {
    fn new(coeffs: [f64;7]) -> Self {
        Self {
            ariana_goertzde: coeffs.map(|c| (c, SharedRb::<Heap<f64>>::new(2)))
        }
    }

    fn push(self: &mut Self, sample: f64) {
        for (coeff, ring) in &mut self.ariana_goertzde {
            let mut riter = ring.iter();
            let q2 = *riter.next().unwrap_or(&0.0);
            let q1 = *riter.next().unwrap_or(&0.0);
            ring.push_overwrite(*coeff * q1 - q2 + sample);
        }
    }

    fn goertzel_me(self: &Self) -> Vec<f64> {
        self.ariana_goertzde.iter().map(|(coeff, ring)| {
            let mut riter = ring.iter();
            let q2 = *riter.next().unwrap_or(&0.0);
            let q1 = *riter.next().unwrap_or(&0.0);
            q1*q1 + q2*q2 - q1*q2*coeff
        }).collect()
    }
}


fn main() {
    let mag_threshold = 26.5_f64.exp();
    let coeffs = FREQS.map(|f| goertzel_coeff(f, SAMPLE_FREQ));

    let mut args = Arguments::from_env();
    let fname = args.value_from_str::<_, String>("-f").unwrap();

    let host = cpal::default_host();
    let device = host.default_output_device().unwrap();
    let supported_config = device
        .supported_output_configs()
        .unwrap()
        .filter_map(|r| {
            if r.channels() == 2 && r.sample_format() == SampleFormat::F32 {
                r.try_with_sample_rate(cpal::SampleRate(SAMPLE_FREQ))
            } else {
                None
            }
        })
        .next()
        .unwrap();

    let in_device = host.default_input_device().unwrap();
    let in_config: SupportedStreamConfig = in_device
        .supported_input_configs()
        .unwrap()
        .map(|r| dbg!(r))
        .filter_map(|r| if r.channels() == 2 && r.sample_format() == SampleFormat::F32 {
            r.try_with_sample_rate(cpal::SampleRate(SAMPLE_FREQ))
        } else {
            None
        }).next().unwrap();
    let (send_input_ch, rcv_input_ch) = channel::<f32>();
    let in_stream = in_device.build_input_stream(
        &in_config.into(),
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            for sample in data.iter() {
                send_input_ch.send(*sample).unwrap();
            }
        },
        move |_| { dbg!("Fuck error handling"); },
        None,
    ).unwrap();

    let reader = hound::WavReader::open(fname).unwrap();
    let mut samples = reader.into_samples::<i32>();

    let (send_ch, rcv_ch) = channel::<f32>();
    let mut playback_idx = 0;
    let out_stream = device.build_output_stream(
        &supported_config.into(),
        move |data: &mut[f32], _: &cpal::OutputCallbackInfo| {
            for sample in data.iter_mut() {
                let next_sample = rcv_input_ch.recv().unwrap();
                //dbg!(next_sample);
                if playback_idx % 2 == 0 {
                    send_ch.send(next_sample * 2.0_f32.powf(16.0)).unwrap();
                }
                *sample = next_sample as f32; // / 2.0_f32.powf(16.0);

                playback_idx += 1;
            }
        },
        move |_| { dbg!("Fuck error handling"); },
        None,
    ).unwrap();

    in_stream.play().unwrap();
    out_stream.play().unwrap();

    let mut sample_idx = 0;
    let mut goertzeler = Goertzeler::new(coeffs);
    let mut last_digit = 0;
    while let Ok(sample) = rcv_ch.recv_timeout(Duration::from_millis(100)) {
        goertzeler.push(sample as f64);
        if sample_idx >= CHUNK_SIZE && sample_idx % WINDOW_INTERVAL == 0 {
            let active_freqs: Vec<_> = goertzeler.goertzel_me()
                .iter()
                .enumerate()
                .filter_map(|(idx, &mag)| (mag > mag_threshold).then_some(idx))
                .collect();
            if active_freqs.len() > 0 {
                //dbg!(&active_freqs);
            }
            // TODO: Move this code into an "identify digit" function
            let digit = match active_freqs[..] {
                [f1, f2] if f2 > 3 => f1*3 + f2-3,
                _ => 0,
            };
            if digit != 0 && digit != last_digit {
                dbg!(digit);
            }
            last_digit = digit;

            goertzeler = Goertzeler::new(coeffs);
        }

        sample_idx += 1;
    }
}
