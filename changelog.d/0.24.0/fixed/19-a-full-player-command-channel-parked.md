- **A full player command channel parked a tokio worker.** `send_cmd` used crossbeam's blocking
  `send` on a bounded(16) channel while the player can sit in `start_playback` for about a second
  during a device rate change. It now waits 250ms and reports "player busy".
