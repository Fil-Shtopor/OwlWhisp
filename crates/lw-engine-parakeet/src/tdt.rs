//! Token-and-Duration Transducer (TDT) greedy decoder, using the fused `decoder_joint` ONNX on CPU.
//!
//! This ports NeMo's `GreedyTDTInfer` and matches the reference pipeline verified on-device
//! (WER ≈ 5 %). Per encoder frame `t`, run the prediction network + joint on the last emitted token
//! (blank at start), split the joint logits into `vocab` token logits and `n_durations` duration
//! logits, argmax each, emit the token if non-blank (advancing the LSTM state only then), and skip
//! `duration` encoder frames. Blank with duration 0, or exceeding `MAX_SYMBOLS_PER_FRAME`, forces a
//! one-frame advance so decoding always progresses.

use lw_ort::ort::session::Session;
use lw_ort::ort::value::Tensor;

use crate::vocab::Vocab;
use crate::{Error, Result};

const MAX_SYMBOLS_PER_FRAME: usize = 10;
/// LSTM prediction-network hidden size (Parakeet v3).
const PRED_HIDDEN: usize = 640;
const PRED_LAYERS: usize = 2;

/// One emitted token with the encoder-frame index it was produced at.
#[derive(Clone, Copy, Debug)]
pub struct Emission {
    /// Token id.
    pub token: usize,
    /// Encoder frame index (multiply by 0.08 s for a timestamp).
    pub frame: usize,
}

/// The TDT decoder holds the fused decoder+joint session.
pub struct TdtDecoder {
    session: Session,
    blank_id: usize,
    num_tokens: usize,
    num_durations: usize,
}

impl TdtDecoder {
    /// Create from a loaded `decoder_joint` session. `vocab` gives the blank id and token count.
    pub fn new(session: Session, vocab: &Vocab) -> Self {
        // joint output is vocab (incl. blank) + n_durations; v3: 8193 + 5 = 8198.
        let num_tokens = vocab.len();
        Self {
            session,
            blank_id: vocab.blank_id(),
            num_tokens,
            num_durations: 5,
        }
    }

    /// Greedy-decode an encoder output of shape `[1, D, T]` (row-major flattened, `d*T + t`),
    /// with `enc_len` valid frames. `enc_dim` is D (1024). Returns the emitted tokens.
    pub fn decode(&mut self, encoder_out: &[f32], enc_dim: usize, enc_len: usize) -> Result<Vec<Emission>> {
        let t_total = if enc_dim == 0 {
            0
        } else {
            encoder_out.len() / enc_dim
        };
        let end = enc_len.min(t_total);
        let mut emissions = Vec::new();

        // LSTM states, zero-initialized: [2, 1, 640] each.
        let mut state1 = vec![0.0f32; PRED_LAYERS * PRED_HIDDEN];
        let mut state2 = vec![0.0f32; PRED_LAYERS * PRED_HIDDEN];
        let mut last_token = self.blank_id as i32;

        let mut t = 0usize;
        while t < end {
            // Slice encoder frame t into [1, D, 1].
            let mut frame = vec![0.0f32; enc_dim];
            for d in 0..enc_dim {
                frame[d] = encoder_out[d * t_total + t];
            }
            let mut symbols = 0usize;
            loop {
                let (token, duration, s1, s2) = self.step(&frame, enc_dim, last_token, &state1, &state2)?;
                if token == self.blank_id {
                    t += duration.max(1);
                    break;
                }
                emissions.push(Emission { token, frame: t });
                last_token = token as i32;
                state1 = s1;
                state2 = s2;
                symbols += 1;
                if duration > 0 {
                    t += duration;
                    break;
                }
                if symbols >= MAX_SYMBOLS_PER_FRAME {
                    t += 1;
                    break;
                }
            }
        }
        Ok(emissions)
    }

    /// One decoder+joint step. Returns `(token, duration, new_state1, new_state2)`.
    fn step(
        &mut self,
        frame: &[f32],
        enc_dim: usize,
        last_token: i32,
        state1: &[f32],
        state2: &[f32],
    ) -> Result<(usize, usize, Vec<f32>, Vec<f32>)> {
        let enc = Tensor::from_array((vec![1i64, enc_dim as i64, 1i64], frame.to_vec()))?;
        let targets = Tensor::from_array((vec![1i64, 1i64], vec![last_token]))?;
        let target_len = Tensor::from_array((vec![1i64], vec![1i32]))?;
        let st1 = Tensor::from_array((
            vec![PRED_LAYERS as i64, 1i64, PRED_HIDDEN as i64],
            state1.to_vec(),
        ))?;
        let st2 = Tensor::from_array((
            vec![PRED_LAYERS as i64, 1i64, PRED_HIDDEN as i64],
            state2.to_vec(),
        ))?;

        let outputs = self.session.run(lw_ort::ort::inputs![
            "encoder_outputs" => enc,
            "targets" => targets,
            "target_length" => target_len,
            "input_states_1" => st1,
            "input_states_2" => st2,
        ])?;

        let logits = outputs["outputs"].try_extract_array::<f32>()?;
        let flat: Vec<f32> = logits.iter().copied().collect();
        // The joint output is [..., num_tokens + num_durations]; the last dim is what we split.
        let total = self.num_tokens + self.num_durations;
        if flat.len() < total {
            return Err(Error::Decode(format!(
                "joint output too small: {} < {}",
                flat.len(),
                total
            )));
        }
        // Take the final `total` values (batch/time dims are 1).
        let base = flat.len() - total;
        let token_logits = &flat[base..base + self.num_tokens];
        let dur_logits = &flat[base + self.num_tokens..base + total];
        let token = argmax(token_logits);
        let duration = argmax(dur_logits); // duration index == number of frames to skip

        let new_s1 = extract_state(&outputs, "output_states_1", state1);
        let new_s2 = extract_state(&outputs, "output_states_2", state2);
        Ok((token, duration, new_s1, new_s2))
    }
}

fn extract_state(
    outputs: &lw_ort::ort::session::SessionOutputs<'_>,
    name: &str,
    fallback: &[f32],
) -> Vec<f32> {
    match outputs.get(name).and_then(|v| v.try_extract_array::<f32>().ok()) {
        Some(arr) => arr.iter().copied().collect(),
        None => fallback.to_vec(),
    }
}

fn argmax(v: &[f32]) -> usize {
    let mut best = 0usize;
    let mut best_val = f32::NEG_INFINITY;
    for (i, &x) in v.iter().enumerate() {
        if x > best_val {
            best_val = x;
            best = i;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argmax_picks_max() {
        assert_eq!(argmax(&[0.1, 0.9, 0.3]), 1);
        assert_eq!(argmax(&[-1.0, -2.0, -0.5]), 2);
    }

    #[test]
    fn argmax_empty_is_zero() {
        assert_eq!(argmax(&[]), 0);
    }
}
