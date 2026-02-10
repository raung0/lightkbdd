# lightkbdd

A keyboard backlight daemon for Linux that automatically dims the keyboard backlight after a period of inactivity.

## Features

- Configurable timing for keyboard backlight dimming
- Smooth fade-in and fade-out transitions
- Low resource usage

## Usage

```
Usage: lightkbdd [OPTIONS]

Options:
  -i, --idle <IDLE_MS>          Keyboard idle time in milliseconds [default: 10000]
  -O, --fade-out <FADE_OUT_MS>  Keyboard fade out time in milliseconds [default: 800]
  -I, --fade-in <FADE_IN_MS>    Keyboard fade in time in milliseconds [default: 250]
  -v, --verbose
  -h, --help                    Print help
```

## Building from source

Requires Rust 1.85 or later:

```bash
cargo build --release
```

The binary will be available at `target/release/lightkbdd`.

## NixOS Installation

### Using the NixOS module

Add this flake to your system configuration:

```nix
{
  inputs.lightkbdd.url = "github:raung0/lightkbdd";

  outputs = { self, nixpkgs, lightkbdd, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        lightkbdd.nixosModules.default
        {
          nixpkgs.overlays = [ lightkbdd.overlays.default ];
          services.lightkbdd = {
            enable = true;
            extraArgs = [ "--idle" "15000" "--fade-out" "1000" "--fade-in" "300" ];
          };
        }
      ];
    };
  };
}
```

### Trying it out temporarily

To try lightkbdd without installing it permanently:

```bash
sudo nix run github:raung0/lightkbdd
```

## License

This project is licensed under the GPL-v3. For more information, consult the [LICENSE](LICENSE) file.

