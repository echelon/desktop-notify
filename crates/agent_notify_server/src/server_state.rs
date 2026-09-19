use std::sync::{Arc, Mutex};

use crate::audio_player::AudioPlayerHandle;
use crate::config::NotifyConfig;
use crate::notifications::NotificationState;

#[derive(Clone)]
pub struct ServerState {
  pub config: Arc<NotifyConfig>,
  pub audio: AudioPlayerHandle,
  pub notifications: Arc<Mutex<NotificationState>>,
}
