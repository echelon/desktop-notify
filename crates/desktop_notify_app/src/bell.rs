//! Frames for the tray bell's alert animation: a gentle gold swing with sound
//! waves. They are drawn from the one tray icon at startup, so `icons-src/` and
//! `icons/` stay the only bell artwork. This module is pure so it can be tested
//! without a GUI.

use std::f32::consts::{FRAC_PI_2, TAU};
use std::time::Duration;

/// Peak tilt each way.
const MAX_TILT_DEGREES: f32 = 12.0;
/// One full left-and-right swing takes `FRAME_COUNT * FRAME_INTERVAL` (1.6 s).
/// Must be even, so that both centered positions are frames.
const FRAME_COUNT: usize = 24;
pub const FRAME_INTERVAL: Duration = Duration::from_millis(66);
/// Frames keep their own color, so this must stay legible on both light and dark
/// menu bars, which a template image would otherwise handle for us.
const GOLD: [u8; 3] = [0xE6, 0xB9, 0x4A];

/// When each arc is brightest within a half-swing, and for how many frames either
/// side of that it is visible. Both arcs are gone by the half-swing's end, so
/// centered frames carry no waves and stopping there never cuts one off.
const INNER_WAVE_FRAME: f32 = 7.0;
const OUTER_WAVE_FRAME: f32 = 9.0;
const WAVE_FADE_FRAMES: f32 = 3.0;
/// The outer arc is fainter, as if the sound were dying away.
const OUTER_WAVE_OPACITY: f32 = 0.65;
const WAVE_SPAN_DEGREES: f32 = 25.0;

/// The swing's images, wider than the icon because the waves sit beside the bell.
pub struct SwingFrames {
  pub width: u32,
  pub height: u32,
  pub frames: Vec<Vec<u8>>,
}

/// Draws one full swing. Frame 0 is the upright bell.
pub fn swing_frames(rgba: &[u8], width: u32, height: u32) -> SwingFrames {
  let (padded, padded_width) = pad(rgba, width as usize, height as usize);
  let height = height as usize;
  let frames = (0..FRAME_COUNT)
    .map(|frame| {
      let mut image = tilted(&padded, padded_width, height, frame);
      let waves = waves(padded_width, height, frame);
      for (pixel, wave) in image.chunks_exact_mut(4).zip(waves) {
        let bell = pixel[3] as f32 / 255.0;
        pixel[..3].copy_from_slice(&GOLD);
        pixel[3] = ((bell + wave * (1.0 - bell)) * 255.0).round() as u8;
      }
      image
    })
    .collect();
  SwingFrames {
    width: padded_width as u32,
    height: height as u32,
    frames,
  }
}

/// Where the animation goes next; `None` is the plain resting icon. A bell told
/// to stop finishes its current half-swing, rather than snapping back mid-tilt.
pub fn next_position(position: Option<usize>, swinging: bool) -> Option<usize> {
  match position {
    None => swinging.then_some(1),
    Some(frame) if !swinging && frame % (FRAME_COUNT / 2) == 0 => None,
    Some(frame) => Some((frame + 1) % FRAME_COUNT),
  }
}

/// Adds a quarter of the icon's width to each side.
fn pad(rgba: &[u8], width: usize, height: usize) -> (Vec<u8>, usize) {
  let margin = width / 4;
  let padded_width = width + 2 * margin;
  let mut padded = vec![0; padded_width * height * 4];
  for y in 0..height {
    let source = &rgba[y * width * 4..(y + 1) * width * 4];
    let start = (y * padded_width + margin) * 4;
    padded[start..start + source.len()].copy_from_slice(source);
  }
  (padded, padded_width)
}

fn tilted(rgba: &[u8], width: usize, height: usize, frame: usize) -> Vec<u8> {
  tilt(rgba, width, height, tilt_degrees(frame))
}

fn tilt_degrees(frame: usize) -> f32 {
  let phase = TAU * frame as f32 / FRAME_COUNT as f32;
  MAX_TILT_DEGREES * phase.sin()
}

/// Maps a position in a tilted image back to the upright bell's coordinates. The
/// rotation is around the bell's crown, as if it hung from it, and positive angles
/// swing the bottom of the bell left.
fn untilt(width: usize, height: usize, degrees: f32) -> impl Fn(f32, f32) -> (f32, f32) {
  let (pivot_x, pivot_y) = (width as f32 * 0.5, height as f32 * 0.125);
  let (sin, cos) = degrees.to_radians().sin_cos();
  move |x, y| {
    let (dx, dy) = (x - pivot_x, y - pivot_y);
    (pivot_x + dx * cos + dy * sin, pivot_y - dx * sin + dy * cos)
  }
}

/// Each output pixel samples the source bilinearly, and anything outside the
/// source is transparent.
fn tilt(rgba: &[u8], width: usize, height: usize, degrees: f32) -> Vec<u8> {
  let untilt = untilt(width, height, degrees);
  let texel = |x: isize, y: isize, channel: usize| -> f32 {
    if x < 0 || y < 0 || x >= width as isize || y >= height as isize {
      0.0
    } else {
      rgba[(y as usize * width + x as usize) * 4 + channel] as f32
    }
  };
  let mut output = vec![0; width * height * 4];
  for y in 0..height {
    for x in 0..width {
      // Pixel centers, to texel coordinates where texel 0 is centered at 0.
      let (source_x, source_y) = untilt(x as f32 + 0.5, y as f32 + 0.5);
      let (source_x, source_y) = (source_x - 0.5, source_y - 0.5);
      let (left, top) = (source_x.floor(), source_y.floor());
      let (right_weight, bottom_weight) = (source_x - left, source_y - top);
      let (left, top) = (left as isize, top as isize);
      for channel in 0..4 {
        let upper = texel(left, top, channel) * (1.0 - right_weight)
          + texel(left + 1, top, channel) * right_weight;
        let lower = texel(left, top + 1, channel) * (1.0 - right_weight)
          + texel(left + 1, top + 1, channel) * right_weight;
        output[(y * width + x) * 4 + channel] =
          (upper * (1.0 - bottom_weight) + lower * bottom_weight).round() as u8;
      }
    }
  }
  output
}

/// Each pixel's wave opacity (0 to 1) for `frame`. Two arcs ripple outward on the
/// side the bell's bottom swings toward, like the clapper striking that side. They
/// are defined in the upright bell's coordinates, so they tilt along with it.
fn waves(width: usize, height: usize, frame: usize) -> Vec<f32> {
  let half_swing = FRAME_COUNT / 2;
  let side = if frame < half_swing { -1.0 } else { 1.0 };
  let time = (frame % half_swing) as f32;
  // Sizes are in icon pixels at the 64-pixel artwork's scale, matching its stroke.
  let scale = height as f32 / 64.0;
  let (center_x, center_y) = (width as f32 * 0.5, 32.0 * scale);
  let arcs = [
    (31.0 * scale, fade(time - INNER_WAVE_FRAME)),
    (
      40.0 * scale,
      fade(time - OUTER_WAVE_FRAME) * OUTER_WAVE_OPACITY,
    ),
  ];
  let half_stroke = 2.0 * scale;
  let untilt = untilt(width, height, tilt_degrees(frame));
  let (span_sin, span_cos) = WAVE_SPAN_DEGREES.to_radians().sin_cos();
  let mut output = vec![0.0; width * height];
  for y in 0..height {
    for x in 0..width {
      let (upright_x, upright_y) = untilt(x as f32 + 0.5, y as f32 + 0.5);
      let outward = (upright_x - center_x) * side;
      let dy = upright_y - center_y;
      if outward <= 0.0 {
        continue;
      }
      let within_span = dy.abs() * span_cos <= outward * span_sin;
      for (radius, opacity) in arcs {
        // Distance to the arc itself, or past its ends to its rounded caps.
        let distance = if within_span {
          (outward.hypot(dy) - radius).abs()
        } else {
          (outward - radius * span_cos).hypot(dy.abs() - radius * span_sin)
        };
        let coverage = (half_stroke + 0.5 - distance).clamp(0.0, 1.0);
        let pixel = &mut output[y * width + x];
        *pixel = f32::max(*pixel, coverage * opacity);
      }
    }
  }
  output
}

/// Eases from 0 through 1 and back to 0 around `frames_from_peak == 0`.
fn fade(frames_from_peak: f32) -> f32 {
  if frames_from_peak.abs() >= WAVE_FADE_FRAMES {
    0.0
  } else {
    (FRAC_PI_2 * frames_from_peak / WAVE_FADE_FRAMES)
      .cos()
      .powi(2)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use tauri::image::Image;

  fn tray_icon() -> Image<'static> {
    Image::from_bytes(include_bytes!("../icons/tray.png")).unwrap()
  }

  fn padded_icon() -> (Vec<u8>, usize, usize) {
    let icon = tray_icon();
    let height = icon.height() as usize;
    let (padded, width) = pad(icon.rgba(), icon.width() as usize, height);
    (padded, width, height)
  }

  fn alphas(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).map(|p| p[3]).collect()
  }

  fn total_alpha(rgba: &[u8]) -> u64 {
    rgba.chunks_exact(4).map(|p| p[3] as u64).sum()
  }

  /// The average column of the visible ink, to tell which way a frame leans.
  fn horizontal_center(rgba: &[u8], width: usize) -> f64 {
    let weighted: f64 = rgba
      .chunks_exact(4)
      .enumerate()
      .map(|(i, p)| (i % width) as f64 * p[3] as f64)
      .sum();
    weighted / total_alpha(rgba) as f64
  }

  #[test]
  fn centered_frames_are_the_upright_bell_without_waves() {
    let icon = tray_icon();
    let (padded, _, _) = padded_icon();
    let swing = swing_frames(icon.rgba(), icon.width(), icon.height());
    assert_eq!(swing.frames.len(), FRAME_COUNT);
    for frame in [0, FRAME_COUNT / 2] {
      let matches = alphas(&swing.frames[frame])
        .iter()
        .zip(alphas(&padded))
        .all(|(a, b)| a.abs_diff(b) <= 1);
      assert!(matches, "frame {frame}");
    }
  }

  #[test]
  fn every_frame_is_entirely_gold_and_the_expected_size() {
    let icon = tray_icon();
    let swing = swing_frames(icon.rgba(), icon.width(), icon.height());
    assert_eq!(swing.height, icon.height());
    assert!(swing.width > icon.width());
    for frame in &swing.frames {
      assert_eq!(frame.len(), (swing.width * swing.height * 4) as usize);
      assert!(frame.chunks_exact(4).all(|p| p[..3] == GOLD));
    }
  }

  #[test]
  fn the_bell_tilts_both_ways_without_clipping() {
    let (padded, width, height) = padded_icon();
    let upright = horizontal_center(&padded, width);
    let left = tilted(&padded, width, height, FRAME_COUNT / 4);
    let right = tilted(&padded, width, height, FRAME_COUNT * 3 / 4);
    assert!(horizontal_center(&left, width) < upright - 2.0);
    assert!(horizontal_center(&right, width) > upright + 2.0);
    // Resampling softens edges slightly, but cut-off ink would lose far more.
    let expected = total_alpha(&padded) as f64;
    for frame in 0..FRAME_COUNT {
      let image = tilted(&padded, width, height, frame);
      assert!((total_alpha(&image) as f64 - expected).abs() / expected < 0.01);
    }
  }

  #[test]
  fn waves_appear_on_the_side_the_bell_swings_toward() {
    let (_, width, height) = padded_icon();
    let ink_by_side = |frame: usize| {
      let waves = waves(width, height, frame);
      let left: f32 = (0..height)
        .flat_map(|y| (0..width / 2).map(move |x| y * width + x))
        .map(|i| waves[i])
        .sum();
      let total: f32 = waves.iter().sum();
      (left, total - left)
    };
    let (left, right) = ink_by_side(FRAME_COUNT / 4 + 1);
    assert!(left > 0.0 && right == 0.0);
    let (left, right) = ink_by_side(FRAME_COUNT * 3 / 4 + 1);
    assert!(left == 0.0 && right > 0.0);
  }

  #[test]
  fn waves_fit_the_canvas_and_clear_the_tilted_bell() {
    let (padded, width, height) = padded_icon();
    for frame in 0..FRAME_COUNT {
      let waves = waves(width, height, frame);
      for y in 0..height {
        assert_eq!(waves[y * width], 0.0, "left edge, frame {frame}");
        assert_eq!(
          waves[y * width + width - 1],
          0.0,
          "right edge, frame {frame}"
        );
      }
      for x in 0..width {
        assert_eq!(waves[x], 0.0, "top edge, frame {frame}");
        assert_eq!(
          waves[(height - 1) * width + x],
          0.0,
          "bottom edge, frame {frame}"
        );
      }
      let bell = alphas(&tilted(&padded, width, height, frame));
      let overlapping = bell
        .iter()
        .zip(&waves)
        .filter(|(bell, wave)| **bell > 64 && **wave > 0.25)
        .count();
      assert_eq!(overlapping, 0, "frame {frame}");
    }
  }

  #[test]
  fn waves_tilt_with_the_bell() {
    let (_, width, height) = padded_icon();
    // Weighted average row of the arcs. An arc level with the ground would sit
    // centered on its own height; tilted with the bell, the leaning side rises.
    let average_row = |frame: usize| {
      let waves = waves(width, height, frame);
      let total: f32 = waves.iter().sum();
      waves
        .iter()
        .enumerate()
        .map(|(i, wave)| (i / width) as f32 * wave)
        .sum::<f32>()
        / total
    };
    let level_row = height as f32 * 0.5;
    for frame in [FRAME_COUNT / 4 + 1, FRAME_COUNT * 3 / 4 + 1] {
      assert!(average_row(frame) < level_row - 3.0, "frame {frame}");
    }
  }

  #[test]
  fn stopping_finishes_the_half_swing_and_returns_to_the_resting_icon() {
    assert_eq!(next_position(None, false), None);
    assert_eq!(next_position(None, true), Some(1));
    assert_eq!(next_position(Some(0), true), Some(1));
    assert_eq!(next_position(Some(FRAME_COUNT - 1), true), Some(0));
    assert_eq!(next_position(Some(1), false), Some(2));
    assert_eq!(
      next_position(Some(FRAME_COUNT / 2 - 1), false),
      Some(FRAME_COUNT / 2)
    );
    assert_eq!(next_position(Some(FRAME_COUNT / 2), false), None);
    assert_eq!(next_position(Some(FRAME_COUNT - 1), false), Some(0));
    assert_eq!(next_position(Some(0), false), None);
  }
}
