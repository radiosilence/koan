//! Plays a short file to its end through the real Player, on the real device.
//! `cargo run -p koan-core --example end_of_queue`
fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    use koan_core::player::Player;
    use koan_core::player::commands::PlayerCommand;
    use koan_core::player::state::{LoadState, PlaybackState, PlaylistItem, QueueItemId};

    let path = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: end_of_queue <audio file>"),
    );

    let (state, _timeline, _viz, tx) = Player::spawn();

    let item = PlaylistItem {
        playlist_entry_id: None,
        id: QueueItemId::new(),
        db_id: None,
        path,
        title: "Short".into(),
        artist: "Test".into(),
        album_artist: "Test".into(),
        album: "Test".into(),
        year: None,
        codec: Some("WAV".into()),
        track_number: Some(1),
        disc: Some(1),
        duration_ms: None,
        load_state: LoadState::Ready,
    };
    let id = item.id;
    tx.send(PlayerCommand::AddToPlaylist(vec![item])).unwrap();
    tx.send(PlayerCommand::Play(id)).unwrap();

    // KOAN_STOP_AFTER_MS: force a teardown instead of waiting for the queue to
    // run out, so callback variants that never drain the ring still get there.
    if let Some(ms) = std::env::var("KOAN_STOP_AFTER_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        println!("sending Stop");
        tx.send(PlayerCommand::Stop).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        println!("SURVIVED an explicit stop");
        return;
    }

    let mut last = PlaybackState::Stopped;
    for _ in 0..25 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let now = state.playback_state();
        if now != last {
            println!("state -> {:?} at {}ms", now, state.position_ms());
            last = now;
        }
    }
    println!("SURVIVED the end of the queue");
}
