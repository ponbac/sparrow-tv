# Keep Android playback while the picture can be seen

A Playback Session on Android lives while the Activity is started, not while it is resumed, so a picture-in-picture window keeps playing. Leaving the app shrinks a playing Media3 picture into Android's picture-in-picture window; the player also has a Picture in picture button.

## Lifecycle boundary

- Picture-in-picture, split screen and a dialog over the app pause the Activity and leave the picture on screen. Only `onStop` means the picture cannot be seen: Home with picture-in-picture off or unavailable, the picture-in-picture window dismissed, the screen turned off.
- `MainActivity` reports `onStart` and `onStop` to Rust through the JNI function `reportPlaybackVisibility` (`app/src-tauri/src/android_lifecycle.rs`). It replaces tao's Suspended and Resumed on Android, which follow the process-wide pause. Rust orders the reports by the time of the call, suspends or resumes the transport as before, and publishes `sparrow://playback-lifecycle` to the page.
- `onStart` reports twice: at once, and again when the WebView has drawn a frame. An event that reaches the WebView while it wakes can be lost, and the page would then stay paused. Rust resumes nothing that is already resumed, and the page ignores the repeated event.
- On Android the page no longer suspends on document visibility. wry pauses the WebView in `onPause`, which would hide the document in picture-in-picture; `MainActivity` resumes the WebView there, and pauses it in `onStop`.
- The installed capability grants `core:event:allow-listen` and `core:event:allow-unlisten`. Without them the page never heard the lifecycle events, and only document visibility had been suspending playback.
- A presentation that the lifecycle suspended reads as paused until the page suspends or stops it itself. The page hears of the suspension by an event that can arrive after its next status read, and a missing presentation would start a recovery.

## Picture-in-picture window

- The window shows the whole Activity. While it lasts, the native picture fills it over an opaque cover, whether or not a presentation is bound, and the page's viewports are not applied. Leaving it puts the picture back where the page last placed it outside picture-in-picture, or shows the page when nothing is bound. The page does not pin the picture's box at the window's small size (`usePicturePin`).
- The aspect ratio follows the video (Media3's size and pixel ratio), within Android's 2.39:1 limit either way; 16:9 until the size is known.
- One button: Pause while playing, Play while paused or after a failure that allows Restart. Play runs the same recovery as the player's own button.
- Android 12 and later enter on their own when the viewer goes Home with a picture playing (`setAutoEnterEnabled`). Android 8 to 11 enter from `onUserLeaveHint`. Android refuses an explicit entry while it switches straight to another app, so switching through Recents or the quick switch on Android 12+ stops playback as before.
- A paused picture does not shrink on its own; the button still works.

## Page and window

- An origin-restricted `WebMessageListener`, `sparrowAndroidPlayer`, is the page's line to the window. Its presence marks the Android app. The page reports `playing`, `paused` or `none` for a Media3 presentation and asks to enter; the window answers with whether picture-in-picture is supported, its mode, a refusal, and Play and Pause commands.
- While the page reports `playing`, the app holds audio focus. Losing focus, or being refused it, pauses as the viewer would; the viewer resumes at the live edge. Android 8 and later duck brief sounds; before that a request to duck pauses too.

## Not done

- No playback without a visible picture: no Android `MediaSession`, media notification or foreground service.
