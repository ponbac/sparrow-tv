# Extend the GPUI Guide to Desktop and Android

The GPUI Guide proved on Shield ([ADR 0009](0009-prove-the-gpui-guide-on-shield-first.md)) becomes the Guide for the installed Linux Desktop app and for Android phones and tablets. All three run one crate, `sparrow-guide-ui`, and differ only in their host and their layout profile. This supersedes the Theater layout of [ADR 0006](0006-use-a-theater-layout-on-desktop-and-number-channels-in-core.md) for installed Desktop and amends [ADR 0007](0007-use-a-pocket-layout-where-the-picture-cannot-be-covered.md) for Pocket on GPUI. ADR 0006's Channel Numbers in core stand.

## Desktop uses the Shield band model

- The picture is embedded in GPUI. An embedded libmpv player delivers decoded frames as `HostEvent::LivePicture`, and the Guide paints them in the picture's rectangle, letterboxed 16:9 on black. There is no external mpv window and no `pictureOverlay` capability.
- The Guide has the Shield's docked picture with its caption. Fullscreen puts the control band under the picture, never over it. There is no separate watch mode.
- Because GPUI paints the picture, it moves with every frame of a flight. The still-capture flight (`PlaybackHost::capture_flight_still`) stays for hosts whose picture is a native surface above GPUI.
- Entering fullscreen fills the app window without toggling desktop window fullscreen.

## Profiles

- The host names its form factor (`GuideHost::form_factor`). Television always uses the shipped 1920-unit canvas, so the Shield's pixels do not change.
- Desktop keeps the television's type and control sizes in layout units and draws them at 0.65, 0.70 or 0.75 logical px per unit (Compact under 1280 px wide, Regular under 1800 px, Wide above). The canvas width, the dock, the band and the number of Now columns follow the window. A class changes only once the window crosses its edge by 24 px.
- Handhelds, and Desktop windows smaller than 900 by 520 px, select a Pocket class from the viewport alone, with no hysteresis: Portrait, Short landscape (wider than tall and under 520 px tall, or within 1050 by 600 px) or Tablet (both sides at least 600 px). Pocket honours the safe-area and keyboard insets.
  - Portrait docks a 256 by 144 unit picture in a band above a toolbar, the lane and a one-column list. A portrait phone that tunes opens the Watch screen: masthead, a full-width stage, the lower third, controls, details and a Channel bar.
  - Short landscape gives the picture and its details a column of 42% of the safe width beside the toolbar, lane and list.
  - Tablets keep the Desktop band model, dock and Now columns inside the safe area, with sheets up to 45% of its width.
- Setup keeps the shipped rows where the canvas fits them. A narrower column sets the headline smaller in up to two lines, and a shorter one lifts the rows so the action row stays on screen.
- `sparrow-tv-guide` receives the board geometry (`BoardMetrics`) from the profile instead of reading television constants, so focus movement steps by the columns on screen.

## Pointer and keyboard

- One listener on the Guide hit-tests the same geometry the views draw, from shared bounds helpers. A click focuses its target without drawing focus and then takes the Enter path, so pointer and remote share one set of rules. A click on the playing Channel goes fullscreen, as Enter does on Android TV. A double click leaves fullscreen only when both presses land on the fullscreen picture.
- The fullscreen control band and the Pocket player chrome (Watch screen, docked details and their controls) take presses on their own elements, and the Watch details scroll themselves. The root listener leaves those areas to them.
- Desktop draws focus only after keyboard input. Hover gets its own quieter state. Scrolling the wheel moves the board and stops focus from pulling it back until the next arrow key.

## Amendments to ADR 0007 for Pocket on GPUI

- Pocket gets the Guide's motion: the arrival wave, the group slide and picture flights. A native picture moves only as a captured still; the box under a live `SurfaceView` never animates.
- The playing row uses the stage wash and stage edge, not amber.
- Tablets in landscape at least 960 dp wide may switch to Timeline. Phones and portrait tablets keep the list, with a time chip in place of the board view switch. A layout without the chip returns the list to the clock, and one with it returns Timeline to Now.
- Turning a playing Watch screen sideways enters fullscreen, and turning back leaves a fullscreen the turn entered. A landscape window under 320 dp tall is the picture-in-picture window and never turns.

## Owner decisions

- Guide preferences reset once when a device moves to a GPUI package. They are not migrated.
- The GPUI Android packages ship without TalkBack support. GPUI has no Android accessibility tree, and the owner accepts that.
- The Hosted Web client keeps its React layouts under ADRs 0006 and 0007.

## Release cutover

- The x86_64 AppImage packages `sparrow-desktop` and embedded libmpv. The
  Android Gradle project in `app/src-shield/android` packages `sparrow-shield`
  and Media3. The Tauri host and standalone system-mpv window adapter are retired.
- Android requires API 26 for the GPUI backend's `libnativewindow`. NDK r29
  builds the universal APK for `arm64-v8a`, `armeabi-v7a`, `x86`, and `x86_64`.
- Both packages take their product version from `app/package.json`. Android
  retains the production application ID `xyz.ponbac.sparrow`, release signing
  identity, and existing app data directory. The development APK installs
  alongside it as `xyz.ponbac.sparrow.shield`. Desktop retains
  `${XDG_DATA_HOME:-$HOME/.local/share}/xyz.ponbac.sparrow`.
