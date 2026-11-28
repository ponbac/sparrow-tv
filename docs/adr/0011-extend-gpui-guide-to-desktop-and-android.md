# Extend the GPUI Guide to Desktop and Android

The GPUI Guide proved on Shield ([ADR 0009](0009-prove-the-gpui-guide-on-shield-first.md)) becomes the Guide for the installed Linux Desktop app and for Android phones and tablets. All three run one crate, `sparrow-guide-ui`, and differ only in their host and their layout profile. This supersedes the Theater layout of [ADR 0006](0006-use-a-theater-layout-on-desktop-and-number-channels-in-core.md) for installed Desktop and amends [ADR 0007](0007-use-a-pocket-layout-where-the-picture-cannot-be-covered.md) for Pocket on GPUI. ADR 0006's Channel Numbers in core stand.

## Desktop uses the Shield band model

- The picture is embedded in GPUI. An embedded libmpv player delivers decoded frames as `HostEvent::LivePicture`, and the Guide paints them in the picture's rectangle, letterboxed 16:9 on black. There is no external mpv window and no `pictureOverlay` capability.
- The Guide has the Shield's docked picture with its caption. Fullscreen puts the control band under the picture, never over it. There is no separate watch mode.
- Because GPUI paints the picture, it moves with every frame of a flight. The still-capture flight (`PlaybackHost::capture_flight_still`) stays for hosts whose picture is a native surface above GPUI.
- Entering fullscreen also makes the window fullscreen, and leaving restores it, unless the window was fullscreen already. If the compositor ends window fullscreen, the picture stays fullscreen inside the window.

## Profiles

- The host names its form factor (`GuideHost::form_factor`). Television always uses the shipped 1920-unit canvas, so the Shield's pixels do not change.
- Desktop keeps the television's type and control sizes in layout units and draws them at 0.65, 0.70 or 0.75 logical px per unit (Compact under 1280 px wide, Regular under 1800 px, Wide above). The canvas width, the dock, the band and the number of Now columns follow the window. A class changes only once the window crosses its edge by 24 px.
- Handhelds, and Desktop windows smaller than 900 by 520 px, select a Pocket class. They use Compact geometry until the Pocket layout is built on GPUI.
- `sparrow-tv-guide` receives the board geometry (`BoardMetrics`) from the profile instead of reading television constants, so focus movement steps by the columns on screen.

## Pointer and keyboard

- One listener on the Guide hit-tests the same geometry the views draw. A click focuses its target without drawing focus and then takes the Enter path, so pointer and remote share one set of rules. A click on the playing Channel goes fullscreen, as Enter does on Android TV.
- Desktop draws focus only after keyboard input. Hover gets its own quieter state. Scrolling the wheel moves the board and stops focus from pulling it back until the next arrow key.

## Amendments to ADR 0007 for Pocket on GPUI

- Pocket gets the Guide's motion: the arrival wave, the group slide and picture flights. A native picture moves only as a captured still; the box under a live `SurfaceView` never animates.
- The playing row uses the stage wash and stage edge, not amber.
- Tablets in landscape at least 960 dp wide may switch to Timeline. Phones and portrait tablets keep the list.

## Owner decisions

- Guide preferences reset once when a device moves to a GPUI package. They are not migrated.
- The GPUI Android packages ship without TalkBack support. GPUI has no Android accessibility tree, and the owner accepts that.
- The Hosted Web client keeps its React layouts under ADRs 0006 and 0007.
