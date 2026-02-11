use std::{
	fs::File,
	io::Read,
	os::{fd::AsFd, unix::fs::FileTypeExt},
	path::{Path, PathBuf},
	time::{Duration, Instant},
};

use clap::Parser;
use nix::{
	fcntl::{FcntlArg, OFlag, fcntl},
	poll::PollTimeout,
	sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags},
};

const MIN_FADE_TICK_MS: u64 = 16; // 60Hz should be plenty fast

#[derive(Parser)]
struct Options {
	#[arg(
		short = 'i',
		long = "idle",
		help = "Keyboard idle time in milliseconds",
		default_value_t = 10_000
	)]
	idle_ms: u64,
	#[arg(
		short = 'O',
		long = "fade-out",
		help = "Keyboard fade out time in milliseconds",
		default_value_t = 800
	)]
	fade_out_ms: u64,
	#[arg(
		short = 'I',
		long = "fade-in",
		help = "Keyboard fade in time in milliseconds",
		default_value_t = 250
	)]
	fade_in_ms: u64,
	#[arg(short = 'v', long = "verbose", default_value_t = false)]
	verbose: bool,
}

fn to_io_err(e: nix::Error) -> std::io::Error {
	std::io::Error::other(e)
}

fn read_int(path: &Path) -> std::io::Result<u32> {
	let s = std::fs::read_to_string(path)?;
	s.trim()
		.parse::<u32>()
		.map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn clamp01(x: f32) -> f32 {
	x.clamp(0.0, 1.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
	a + (b - a) * t
}

fn get_all_input_devices() -> std::io::Result<Vec<PathBuf>> {
	let mut devices = Vec::new();

	for entry in std::fs::read_dir(Path::new("/dev/input"))? {
		let entry = entry?;
		let ft = entry.file_type()?;

		if ft.is_char_device() && entry.file_name().to_string_lossy().starts_with("event") {
			devices.push(entry.path());
		}
	}

	Ok(devices)
}

fn find_keyboard_backlight_dir() -> std::io::Result<PathBuf> {
	let base = Path::new("/sys/class/leds");
	let mut matches: Vec<PathBuf> = Vec::new();

	for entry in std::fs::read_dir(base)? {
		let entry = entry?;
		let path = entry.path();
		if !path.is_dir() {
			continue;
		}

		if entry
			.file_name()
			.to_string_lossy()
			.ends_with("kbd_backlight")
		{
			matches.push(path);
		}
	}

	match matches.len() {
		0 => Err(std::io::Error::new(
			std::io::ErrorKind::NotFound,
			"No LED ending with 'kbd_backlight' found under /sys/class/leds",
		)),
		_ => Ok(matches.remove(0)),
	}
}

struct Backlight {
	brightness_path: PathBuf,
	max_raw: u32,
	last_raw_written: Option<u32>,
}

impl Backlight {
	fn open() -> std::io::Result<Self> {
		let dir = find_keyboard_backlight_dir()?;
		let brightness_path = dir.join("brightness");
		let max_path = dir.join("max_brightness");

		let max_raw = read_int(&max_path)?;

		Ok(Self {
			brightness_path,
			max_raw,
			last_raw_written: None,
		})
	}

	fn read_raw(&self) -> std::io::Result<u32> {
		read_int(&self.brightness_path)
	}

	fn write_raw(&mut self, raw: u32) -> std::io::Result<()> {
		if self.last_raw_written == Some(raw) {
			return Ok(());
		}
		self.last_raw_written = Some(raw);

		std::fs::write(&self.brightness_path, format!("{raw}\n"))
	}

	fn raw_to_f32(&self, raw: u32) -> f32 {
		if self.max_raw == 0 {
			return 0.0;
		}
		clamp01(raw as f32 / self.max_raw as f32)
	}

	fn f32_to_raw(&self, v: f32) -> u32 {
		(clamp01(v) * self.max_raw as f32).round() as u32
	}
}

#[derive(Clone, Copy, Debug)]
struct Fade {
	start: f32,
	target: f32,
	start_at: Instant,
	duration: Duration,
}

impl Fade {
	fn value_at(&self, now: Instant) -> f32 {
		let elapsed = now.saturating_duration_since(self.start_at);
		let t = (elapsed.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0);
		lerp(self.start, self.target, t)
	}

	fn done(&self, now: Instant) -> bool {
		now.saturating_duration_since(self.start_at) >= self.duration
	}
}

struct Fader {
	current: f32,
	fade: Option<Fade>,
	fade_in: Duration,
	fade_out: Duration,
}

impl Fader {
	fn new(current: f32, fade_in: Duration, fade_out: Duration) -> Self {
		Self {
			current: clamp01(current),
			fade: None,
			fade_in,
			fade_out,
		}
	}

	fn set_target(&mut self, now: Instant, t: f32) {
		let t = clamp01(t);
		let cur = self.value(now);

		if (t - cur).abs() < 0.0001 {
			self.current = t;
			self.fade = None;
			return;
		}

		let dur = if t > cur { self.fade_in } else { self.fade_out };
		self.fade = Some(Fade {
			start: cur,
			target: t,
			start_at: now,
			duration: dur,
		});
	}

	fn value(&mut self, now: Instant) -> f32 {
		if let Some(f) = self.fade {
			let v = clamp01(f.value_at(now));
			self.current = v;
			if f.done(now) {
				self.current = f.target;
				self.fade = None;
			}
		}
		self.current
	}

	fn is_fading(&self) -> bool {
		self.fade.is_some()
	}
}

fn ms_to_timeout(ms: i64) -> PollTimeout {
	if ms <= 0 {
		return PollTimeout::from(0u16);
	}

	let capped = ms.min(u16::MAX as i64) as u16;
	PollTimeout::from(capped)
}

fn main() -> std::io::Result<()> {
	let options = Options::parse();

	let mut backlight = Backlight::open()?;
	let initial_raw = backlight.read_raw()?;
	let initial = backlight.raw_to_f32(initial_raw);

	let mut fader = Fader::new(
		initial,
		Duration::from_millis(options.fade_in_ms.max(1)),
		Duration::from_millis(options.fade_out_ms.max(1)),
	);

	let paths = get_all_input_devices()?;
	let ep = Epoll::new(EpollCreateFlags::empty()).map_err(to_io_err)?;

	let mut files = Vec::<File>::new();
	for p in paths {
		let f = match File::open(&p) {
			Ok(f) => f,
			Err(_) => continue,
		};

		let flags =
			OFlag::from_bits_truncate(fcntl(f.as_fd(), FcntlArg::F_GETFL).map_err(to_io_err)?);
		let new_flags = flags | OFlag::O_NONBLOCK;
		fcntl(f.as_fd(), FcntlArg::F_SETFL(new_flags)).map_err(to_io_err)?;

		let idx = files.len() as u64;
		ep.add(
			&f,
			EpollEvent::new(
				EpollFlags::EPOLLIN | EpollFlags::EPOLLERR | EpollFlags::EPOLLHUP,
				idx,
			),
		)
		.map_err(to_io_err)?;

		files.push(f);
	}

	if files.is_empty() {
		return Err(std::io::Error::new(
			std::io::ErrorKind::NotFound,
			"No readable /dev/input/event* devices found",
		));
	}

	let mut ep_events = [EpollEvent::empty(); 64];
	let mut junk = [0u8; 4096];

	let start = Instant::now();
	let mut last_activity = start;

	let mut saved_raw: Option<u32> = None;
	let mut is_dimmed = false;

	loop {
		let now = Instant::now();

		let idle_deadline = last_activity + Duration::from_millis(options.idle_ms);
		let mut next_wake = idle_deadline;

		if fader.is_fading() {
			next_wake = next_wake.min(now + Duration::from_millis(MIN_FADE_TICK_MS));
		} else if is_dimmed {
			next_wake = now + Duration::from_secs(60);
		}

		let timeout_ms = next_wake
			.checked_duration_since(now)
			.map(|d| d.as_millis() as i64)
			.unwrap_or(0);

		let n = ep
			.wait(&mut ep_events, ms_to_timeout(timeout_ms))
			.map_err(to_io_err)?;

		let now = Instant::now();

		if n != 0 {
			for ev in ep_events.iter().take(n) {
				let idx = ev.data() as usize;
				if idx >= files.len() {
					continue;
				}

				let flags = ev.events();
				if flags.contains(EpollFlags::EPOLLERR) || flags.contains(EpollFlags::EPOLLHUP) {
					let _ = ep.delete(&files[idx]);
					continue;
				}

				loop {
					match files[idx].read(&mut junk) {
						Ok(0) => break,
						Ok(_) => continue,
						Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
						Err(_) => {
							let _ = ep.delete(&files[idx]);
							break;
						}
					}
				}
			}

			last_activity = now;

			if is_dimmed {
				if options.verbose {
					println!("Restoring keyboard brightness");
				}
				let restore_raw = saved_raw.take().unwrap_or(initial_raw);
				fader.set_target(now, backlight.raw_to_f32(restore_raw));
				is_dimmed = false;
			}
		}

		if n == 0 {
			let idle_deadline = last_activity + Duration::from_millis(options.idle_ms);

			if !is_dimmed && now >= idle_deadline {
				if saved_raw.is_none() {
					saved_raw = Some(backlight.read_raw().unwrap_or(initial_raw));
				}
				if options.verbose {
					println!("Dimming keyboard");
				}
				fader.set_target(now, 0.0);
				is_dimmed = true;
			}
		}

		let v = fader.value(now);
		let raw = backlight.f32_to_raw(v);
		backlight.write_raw(raw)?;
	}
}
