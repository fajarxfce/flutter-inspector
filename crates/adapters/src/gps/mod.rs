mod link;
pub mod routes;

use inspector_application::{Event, EventSink, GpsCommand, GpsEvent, GpsStatus};
use inspector_domain::{GpsFix, GpsRoute};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

enum Message {
    Command(GpsCommand),
    Shutdown(oneshot::Sender<()>),
}
#[derive(Default)]
pub struct Gps {
    sender: Option<mpsc::Sender<Message>>,
}
impl Gps {
    pub fn handle(&mut self, command: GpsCommand, sink: EventSink) -> Result<(), String> {
        let sender = self.sender.get_or_insert_with(|| {
            let (sender, receiver) = mpsc::channel(16);
            tokio::spawn(actor(receiver, sink));
            sender
        });
        sender
            .try_send(Message::Command(command))
            .map_err(|_| "GPS controls are busy; try again".into())
    }
    pub async fn shutdown(&mut self) {
        if let Some(sender) = self.sender.take() {
            let (done, completed) = oneshot::channel();
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                let _ = sender.send(Message::Shutdown(done)).await;
                let _ = completed.await;
            })
            .await;
        }
    }
}

#[derive(Default)]
struct Motion {
    fix: GpsFix,
    route: Option<GpsRoute>,
    distance: f64,
    speed: f64,
    playing: bool,
    paused: bool,
    repeat: bool,
}
impl Motion {
    fn command(&mut self, command: GpsCommand) -> Result<(), String> {
        match command {
            GpsCommand::SetFix(mut fix) => {
                fix.validate()?;
                fix.speed = 0.0;
                *self = Self {
                    fix,
                    ..Default::default()
                };
            }
            GpsCommand::Drive { heading, speed } => {
                GpsFix {
                    heading,
                    speed,
                    ..self.fix
                }
                .validate()?;
                if speed <= 0.0 {
                    return Err("Set a speed above zero before driving".into());
                }
                self.fix.heading = heading;
                self.fix.speed = speed;
                self.speed = speed;
                self.route = None;
                self.distance = 0.0;
                self.playing = true;
                self.paused = false;
            }
            GpsCommand::Play {
                points,
                speed,
                repeat,
            } => {
                GpsFix { speed, ..self.fix }.validate()?;
                if speed <= 0.0 {
                    return Err("Set a speed above zero before playing a route".into());
                }
                let route = GpsRoute::new(points)?;
                (self.fix.point, self.fix.heading) = route.sample(0.0);
                self.fix.speed = speed;
                self.route = Some(route);
                self.speed = speed;
                self.repeat = repeat;
                self.distance = 0.0;
                self.playing = true;
                self.paused = false;
            }
            GpsCommand::Pause => {
                self.paused = self.playing;
                self.fix.speed = 0.0;
            }
            GpsCommand::Resume => {
                self.paused = false;
                if self.playing {
                    self.fix.speed = self.speed;
                }
            }
            _ => return Err("Start GPS simulation first".into()),
        }
        Ok(())
    }
    fn advance(&mut self, elapsed: Duration) -> bool {
        if !self.playing || self.paused {
            return false;
        }
        if elapsed > Duration::from_secs(3) {
            self.paused = true;
            self.fix.speed = 0.0;
            return true;
        }
        let meters = self.speed * elapsed.as_secs_f64();
        self.distance += meters;
        if let Some(route) = &self.route {
            if self.distance >= route.length() {
                if self.repeat {
                    self.distance %= route.length();
                } else {
                    self.distance = route.length();
                    self.playing = false;
                    self.fix.speed = 0.0;
                }
            }
            (self.fix.point, self.fix.heading) = route.sample(self.distance);
        } else {
            self.fix.point = self.fix.point.moved(meters, self.fix.heading);
        }
        false
    }
}

struct Session {
    link: Option<link::Link>,
    motion: Motion,
    generation: u64,
}
impl Session {
    fn status(&self, busy: bool, error: bool, message: impl Into<String>) -> Event {
        Event::Gps(GpsEvent::Status(GpsStatus {
            generation: self.generation,
            busy,
            active: self.link.is_some(),
            playing: self.motion.playing,
            paused: self.motion.paused,
            fix: self.motion.fix,
            distance: self.motion.distance,
            total: self.motion.route.as_ref().map(GpsRoute::length),
            message: message.into(),
            error,
        }))
    }
    async fn stop(&mut self) -> Result<(), String> {
        self.motion.playing = false;
        self.motion.paused = false;
        self.motion.fix.speed = 0.0;
        if let Some(link) = self.link.take() {
            link.stop().await?;
        }
        Ok(())
    }
    async fn command(&mut self, command: GpsCommand, sink: &EventSink) -> Result<(), String> {
        match command {
            GpsCommand::Start {
                generation,
                serial,
                mut fix,
            } => {
                self.generation = generation;
                fix.validate()?;
                self.stop().await?;
                sink.send(self.status(true, false, "Preparing GPS on device…"))
                    .await;
                self.link = Some(link::Link::start(&serial).await?);
                fix.speed = 0.0;
                self.motion = Motion {
                    fix,
                    ..Default::default()
                };
            }
            GpsCommand::Stop => {
                self.stop().await?;
                return Ok(());
            }
            GpsCommand::Restore { generation, serial } => {
                self.generation = generation;
                self.stop().await?;
                sink.send(self.status(true, false, "Restoring device GPS…"))
                    .await;
                link::restore(&serial).await?;
                return Ok(());
            }
            command => {
                if self.link.is_none() {
                    return Err("Start GPS simulation first".into());
                }
                self.motion.command(command)?;
            }
        }
        self.link
            .as_mut()
            .ok_or("GPS helper unavailable")?
            .fix(self.motion.fix)
            .await
    }
}

async fn actor(mut receiver: mpsc::Receiver<Message>, sink: EventSink) {
    let mut session = Session {
        link: None,
        motion: Motion::default(),
        generation: 0,
    };
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last = Instant::now();
    loop {
        tokio::select! {
            message = receiver.recv() => {
                match message {
                    Some(Message::Command(command)) => {
                        let result = session.command(command, &sink).await;
                        last = Instant::now();
                        match result {
                            Ok(()) => sink.send(session.status(false, false, if session.link.is_some() { "Mock location active" } else { "Device GPS restored" })).await,
                            Err(error) => {
                                let cleanup = session.stop().await.err();
                                let error = cleanup.map_or(error.clone(), |e| format!("{error}. Cleanup: {e}"));
                                sink.send(session.status(false, true, error)).await;
                            }
                        }
                    }
                    Some(Message::Shutdown(done)) => { let _ = session.stop().await; let _ = done.send(()); break; }
                    None => { let _ = session.stop().await; break; }
                }
            }
            _ = interval.tick(), if session.link.is_some() => {
                let now = Instant::now();
                let slept = session.motion.advance(now.duration_since(last));
                last = now;
                let result = session.link.as_mut().expect("active link").fix(session.motion.fix).await;
                if let Err(error) = result {
                    let _ = session.stop().await;
                    sink.send(session.status(false, true, error)).await;
                } else if slept {
                    sink.send(session.status(false, false, "Playback paused after host delay; resume when ready")).await;
                } else {
                    let message = if session.motion.paused { "Paused · holding position" } else if session.motion.playing { "Simulating movement" } else { "Holding position" };
                    let _ = sink.try_send(session.status(false, false, message));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pause_resume_end_and_sleep_do_not_teleport() {
        let mut motion = Motion::default();
        let start = motion.fix.point;
        let end = start.moved(100.0, 90.0);
        motion
            .command(GpsCommand::Play {
                points: vec![start, end],
                speed: 10.0,
                repeat: false,
            })
            .unwrap();
        motion.advance(Duration::from_secs(2));
        assert!((motion.distance - 20.0).abs() < 0.01);
        motion.command(GpsCommand::Pause).unwrap();
        motion.advance(Duration::from_secs(2));
        assert_eq!(motion.distance, 20.0);
        assert_eq!(motion.fix.speed, 0.0);
        motion.command(GpsCommand::Resume).unwrap();
        for _ in 0..5 {
            motion.advance(Duration::from_secs(2));
        }
        assert!(!motion.playing);
        assert!(motion.fix.point.distance_to(end) < 0.01);
        assert_eq!(motion.fix.speed, 0.0);
        motion
            .command(GpsCommand::Drive {
                heading: 0.0,
                speed: 10.0,
            })
            .unwrap();
        assert!(motion.advance(Duration::from_secs(30)));
        assert!(motion.paused);
        assert_eq!(motion.distance, 0.0);
    }
    #[test]
    fn repeat_wraps_and_invalid_speed_preserves_motion() {
        let mut motion = Motion::default();
        let start = motion.fix.point;
        motion
            .command(GpsCommand::Play {
                points: vec![start, start.moved(5.0, 0.0)],
                speed: 10.0,
                repeat: true,
            })
            .unwrap();
        motion.advance(Duration::from_millis(700));
        assert!((motion.distance - 2.0).abs() < 0.01);
        assert!(motion.playing);
        assert!(
            motion
                .command(GpsCommand::Drive {
                    heading: 0.0,
                    speed: f64::NAN
                })
                .is_err()
        );
        assert!(motion.fix.point.validate().is_ok());
    }
}
