use std::{
	collections::HashMap,
	fs::File,
	io::Read,
	os::{
		fd::{AsFd, AsRawFd},
		unix::fs::FileTypeExt,
	},
	path::{Path, PathBuf},
};

use chrono::Utc;
use clap::Parser;
use nix::{
	fcntl::{fcntl, FcntlArg, OFlag},
	poll::PollTimeout,
	sys::epoll::{Epoll, EpollCreateFlags, EpollEvent, EpollFlags},
};

const KEYBOARD_BACKLIGHT_PATH: &'static str = "/sys/class/leds/kbd_backlight/";

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
}

fn read_int(path: &Path) -> std::io::Result<u32> {
	let s = std::fs::read_to_string(path)?;
	Ok(s.trim()
		.parse::<u32>()
		.map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?)
}

fn write_int(path: &Path, v: u32) -> std::io::Result<()> {
	std::fs::write(path, format!("{v}\n"))
}

fn get_raw_backlight_value() -> std::io::Result<u32> {
	read_int(Path::new(&format!("{KEYBOARD_BACKLIGHT_PATH}/brightness")))
}

fn get_raw_backlight_max_value() -> std::io::Result<u32> {
	read_int(Path::new(&format!(
		"{KEYBOARD_BACKLIGHT_PATH}/max_brightness"
	)))
}

fn get_backlight_value() -> std::io::Result<f32> {
	let max = get_raw_backlight_max_value()? as f32;
	let v = get_raw_backlight_value()? as f32;
	Ok(v / max)
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

fn to_io_err(e: nix::Error) -> std::io::Error {
	std::io::Error::new(std::io::ErrorKind::Other, e)
}

fn clamp01(x: f32) -> f32 {
	if x < 0.0 {
		0.0
	} else if x > 1.0 {
		1.0
	} else {
		x
	}
}

struct Fader {
	max_raw: u32,
	current: f32,
	target: f32,
	last_raw_written: Option<u32>,
	last_tick_ms: i64,
	fade_in_ms: i64,
	fade_out_ms: i64,
}

impl Fader {
	fn new(max_raw: u32, current: f32, now_ms: i64, fade_in_ms: i64, fade_out_ms: i64) -> Self {
		Self {
			max_raw,
			current: clamp01(current),
			target: clamp01(current),
			last_raw_written: None,
			last_tick_ms: now_ms,
			fade_in_ms: fade_in_ms,
			fade_out_ms: fade_out_ms,
		}
	}

	fn set_target(&mut self, t: f32) {
		self.target = clamp01(t);
	}

	fn tick(&mut self, now_ms: i64) -> std::io::Result<()> {
		let dt_ms = now_ms - self.last_tick_ms;
		self.last_tick_ms = now_ms;

		if dt_ms <= 0 {
			return Ok(());
		}

		let diff = self.target - self.current;
		if diff.abs() < 0.0001 {
			return Ok(());
		}

		let dur = if diff > 0.0 {
			self.fade_in_ms
		} else {
			self.fade_out_ms
		};
		let dur = dur.max(1) as f32;

		let step = (dt_ms as f32) / dur;
		if diff > 0.0 {
			self.current = (self.current + step).min(self.target);
		} else {
			self.current = (self.current - step).max(self.target);
		}
		self.current = clamp01(self.current);

		let raw = (self.current * self.max_raw as f32).round() as u32;
		if self.last_raw_written == Some(raw) {
			return Ok(());
		}
		self.last_raw_written = Some(raw);

		write_int(
			Path::new(&format!("{KEYBOARD_BACKLIGHT_PATH}/brightness")),
			raw,
		)
	}
}

fn main() -> std::io::Result<()> {
	let options = Options::parse();

	let paths = get_all_input_devices()?;

	let ep = Epoll::new(EpollCreateFlags::empty()).map_err(to_io_err)?;

	let mut files = Vec::<File>::new();
	let mut fd_to_idx = HashMap::<i32, usize>::new();

	for (i, p) in paths.iter().enumerate() {
		let f = File::open(p)?;
		let fd = f.as_raw_fd();

		let flags =
			OFlag::from_bits_truncate(fcntl(f.as_fd(), FcntlArg::F_GETFL).map_err(to_io_err)?);
		let new_flags = flags | OFlag::O_NONBLOCK;
		fcntl(f.as_fd(), FcntlArg::F_SETFL(new_flags)).map_err(to_io_err)?;

		fd_to_idx.insert(fd, i);

		ep.add(&f, EpollEvent::new(EpollFlags::EPOLLIN, fd as u64))
			.map_err(to_io_err)?;

		files.push(f);
	}

	let mut ep_events = [EpollEvent::empty(); 64];
	let mut junk = [0u8; 4096];

	let now_ms = Utc::now().timestamp_millis();
	let max_raw = get_raw_backlight_max_value()?;
	let initial = get_backlight_value()?;
	let mut fader = Fader::new(
		max_raw,
		initial,
		now_ms,
		options.fade_in_ms as i64,
		options.fade_out_ms as i64,
	);

	let mut last_key_event = now_ms;
	let mut saved_brightness: Option<f32> = None;
	let mut is_dimmed = false;

	loop {
		let n = ep
			.wait(&mut ep_events, PollTimeout::from(10u16))
			.map_err(to_io_err)?;

		let now = Utc::now().timestamp_millis();

		let diff = now - last_key_event;
		if diff > options.idle_ms as i64 && !is_dimmed {
			if saved_brightness.is_none() {
				saved_brightness = Some(fader.current.max(0.01));
			}
			fader.set_target(0.0);
			is_dimmed = true;
		}

		if n != 0 {
			for ev in ep_events.iter().take(n) {
				let fd = ev.data() as i32;
				let Some(&idx) = fd_to_idx.get(&fd) else {
					continue;
				};

				loop {
					match files[idx].read(&mut junk) {
						Ok(0) => break,
						Ok(_) => continue,
						Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
						Err(e) => return Err(e),
					}
				}
			}

			last_key_event = now;

			if is_dimmed {
				let restore = saved_brightness.take().unwrap_or(1.0);
				fader.set_target(restore);
				is_dimmed = false;
			}
		}

		fader.tick(now)?;
	}
}
