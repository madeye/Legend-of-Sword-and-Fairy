# Offscreen Autoplay

The native game exposes an opt-in HTTP interface for automation. It can run
without a visible window or audio while continuing to render frames for an
autoplay client:

```shell
cargo run --release -- --ui-driver --offscreen
```

The default endpoint is `http://127.0.0.1:8765`. Use
`--ui-driver=127.0.0.1:PORT` to select another loopback port. Non-loopback
addresses are rejected.

## Interface

| Request | Purpose |
| --- | --- |
| `GET /v1/status` | Return `status`, logical frame size, and monotonically increasing `frame_id`. |
| `GET /v1/frame.png` | Capture the latest logical 320×200 RGBA frame as PNG. |
| `POST /v1/input/{key}/tap` | Press and release one game key. |
| `POST /v1/input/{key}/press` | Hold one game key down. |
| `POST /v1/input/{key}/release` | Release one held game key. |

Supported key names are `up`, `down`, `left`, `right`, `menu`, `confirm`,
`space`, `page_up`, `page_down`, `home`, `end`, `repeat`, `auto`, `defend`,
`use_item`, `throw_item`, `flee`, `force`, and `status`.

For example, capture a checkpoint, advance dialogue, then walk:

```shell
mkdir -p autoplay-captures
curl http://127.0.0.1:8765/v1/frame.png \
  -o autoplay-captures/001-before-dialogue.png
curl -X POST http://127.0.0.1:8765/v1/input/confirm/tap
curl -X POST http://127.0.0.1:8765/v1/input/down/press
sleep 1
curl -X POST http://127.0.0.1:8765/v1/input/down/release
curl http://127.0.0.1:8765/v1/frame.png \
  -o autoplay-captures/002-after-walk.png
```

An autoplay client can poll `/v1/status`, fetch a frame after `frame_id`
changes, decide its next action from the image, submit input, and save
milestone frames. Physical keyboard input continues to work through the same
engine input path.

## Captured demo

These frames were captured from one silent offscreen run using only the HTTP
interface.

| Main menu | Opening dialogue |
| --- | --- |
| ![Main menu](../screenshots/autoplay/01-main-menu.png) | ![Opening dialogue](../screenshots/autoplay/02-opening-dialogue.png) |

| Free movement | Leaving the bedroom |
| --- | --- |
| ![Free movement after the opening conversation](../screenshots/autoplay/03-free-movement.png) | ![Leaving the starting bedroom](../screenshots/autoplay/04-left-bedroom.png) |

| Corridor encounter | Navigating toward the stairs |
| --- | --- |
| ![Encounter in the upstairs corridor](../screenshots/autoplay/05-corridor-encounter.png) | ![Autoplay navigating the upstairs corridor](../screenshots/autoplay/06-stair-navigation.png) |
