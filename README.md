# miidrop

Library and frontend to send and receive Miis from a 3DS or Wii U, using Mii Maker's local wireless transfer.

Miidrop joins or hosts a Mii Maker session directly over UDS, no homebrew is needed on a console.

<!-- TODO screenshot: Connect page with a console found -->
<!-- TODO screenshot: Library page and a Mii's details -->

## Features

- Find consoles waiting in Mii Maker and join them.
- Host a session that a 3DS searching for players can join.
- Send a Mii to a console and save the Miis it sends back.
- Keep a library of Miis as `.cfsd` files, with import and export.
- Show every field stored in a Mii.
- Optional Mii pictures from a [Mii Renderer](https://mii-unsecure.ariankordi.net) server.

## Requirements

- Linux.
- A wireless adapter that supports monitor mode, frame injection and active monitor mode (for example one using the `ath9k` driver).
- `iw`.
- An `aes_keys.txt` dumped from your own 3DS with GodMode9. It needs `slot0x2DKeyN` and `slot0x31KeyX`/`slot0x31KeyY`.
  Hosting also needs a `udsBeaconKey=` line in the same file.
  No keys are included with miidrop.

For the desktop app:

- Python 3.10 or newer.
- GTK 4, libadwaita 1.5 or newer, PyGObject.
- polkit.

## Building

```sh
cargo build --release
```

The Python module used by the app:

```sh
cd crates/miidrop-py
maturin develop --release
```

## Installing the app

The app runs as your user and starts `miidrop serve` when it needs the radio.

```sh
sudo install -Dm755 target/release/miidrop /usr/bin/miidrop
sudo install -Dm644 app/data/io.github.finthetechguy.Miidrop.policy /usr/share/polkit-1/actions/io.github.finthetechguy.Miidrop.policy
sudo install -Dm644 app/data/io.github.finthetechguy.Miidrop.desktop /usr/share/applications/io.github.finthetechguy.Miidrop.desktop
pip install ./crates/miidrop-py ./app
```

Then start Miidrop GUI and set the key file in 'Preferences'.

## Command line

Radio commands need root, or `CAP_NET_ADMIN` and `CAP_NET_RAW`. The key file comes from `--keys` or `MIIDROP_KEYS`.

```sh
# List consoles waiting in Mii Maker
sudo miidrop scan -i wlan1

# Send a Mii to a waiting console
sudo miidrop --keys aes_keys.txt send -i wlan1 mii.cfsd

# Save a Mii a waiting console sends
sudo miidrop --keys aes_keys.txt receive -i wlan1

# Host and wait for a searching 3DS instead
sudo miidrop --keys aes_keys.txt receive -i wlan1 --wait

# Show a Mii file as JSON
miidrop info mii.cfsd
```

`miidrop --help` lists every command and option.

## Mii pictures

Pictures are off by default. When turned on in 'Preferences', each Mii's data is sent to the picture server to render it.
The server address can be changed to your own instance. Pictures are cached in `~/.cache/miidrop/pictures`.

## License

[MIT](LICENSE).

miidrop is not affiliated with Nintendo. Mii, Nintendo 3DS and Wii U are trademarks of Nintendo.
