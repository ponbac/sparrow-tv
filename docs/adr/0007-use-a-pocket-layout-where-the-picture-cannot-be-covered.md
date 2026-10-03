# Use a pocket layout where the picture cannot be covered

Every window that is not Theater uses the pocket layout. It has Theater's two modes, turned for a phone: watch mode gives the window to the playing Channel, guide mode docks the picture to a small band above the guide. Nothing is ever drawn over the picture, the guide is a list of what each Channel has on instead of a timeline, and the picture's box changes size only when the picture can follow it. This amends [ADR 0006](0006-use-a-theater-layout-on-desktop-and-number-channels-in-core.md), whose stacked layout it replaces.

## Where it applies

- The rule that chooses the layout is unchanged: Theater needs the `pictureOverlay` capability and either a window of at least 1051 by 601 px or a fullscreen document root. Everything else is pocket: Android at every size, smaller hosted and Linux windows, and the installed app until its capability read answers.
- Pocket draws nothing over the picture on any of them, so one layout serves the device that cannot be covered and the windows that are merely small.

## Modes

- Watch mode, from the top: the masthead (wordmark, a search button, the clock); the picture across the window, `min(56.25vw, 52dvh)` tall, and less in a window too short for that, down to the band's height, so the lines and controls under it keep their room; a lower third under it with the Channel Number, the Channel's name and Channel Group, the quality switch, the live Programme's title and its progress; one row of icon controls; the Programme's description and up to four Programmes that follow on this Channel; and a bar pinned to the bottom with the previous Channel, Guide and the next Channel. Only the description and what follows scroll. That block is kept out of sight until the schedule read that carries the description has answered, so it appears once and in its place: after a change of Channel the lines above it follow at once and the block a moment later.
- Guide mode docks the picture to a band at the top left, 9.375rem by 5.25rem, with the Channel Number, name, title, progress and minutes left beside it. The guide fills the rest. A button over those lines and a tap on the picture both return to watch mode.
- While nothing plays there is no picture box. The guide has the window under the masthead.
- A player that could not load says so in the picture's box, with a button to reload and one to close it. In the band there is room for the heading only, in small type from the top left; a tap on the band returns to watch mode, where the sentence and the buttons are. Where the box is lower than the notice, the notice starts at its top and scrolls.
- Tuning a Channel enters watch mode, whether from a list row, a quality chip, a search result or Agent Control. Guide mode is forced while nothing plays and while the picture is in the external mpv window, as in Theater. With mpv the controls take a row of their own under the band, because there is no watch mode to reach them from.
- The masthead's search button opens the guide and puts focus in its search field.
- The bar's previous and next buttons are the arrow keys of Theater: the lower third shows the target at once and the Channel is tuned 350 ms after the last press. They move through the 21 rows read around the playing Channel and skip hidden Channel Groups. A row at the edge of those rows may lack some of its Quality Variants, so it is not offered. Where the Channel shown has nothing to move to on one side and the Channel Catalog goes on there, the client reads a hundred rows around the edge row and adds what lies beyond, again and again up to eight reads. So a run of presses carries on past the rows first read, and a hidden Channel Group of a few hundred Channels does not end the bar. Theater's arrow keys do the same from their 61 rows.
- Pocket has no keys of its own. F still toggles fullscreen on the focused player.
- The controls that change mode leave the layout when they are used, so focus moves with them: from the bar's Guide button to the band's button, and from the band's button to the picture. A zap from the bar leaves focus on its button; any other tune rests it on the picture.

## One tree, sized by attributes

- Pocket and Theater are the same React tree. The shell carries `data-layout`, `data-mode` and `data-chrome` as before, and now `data-playing`, `data-dock` and `data-external` in both layouts. `app/src/features/stage/pocket.css` holds every pocket rule.
- The mode that is not shown stays mounted. In watch mode the guide, with its search field, Channel Group chips, list rows and status readouts, is taken out of the layout by CSS; in guide mode the controls, the description block and the channel bar are. Nothing is unmounted and nothing gets the `hidden` attribute, so a script can press a guide row while a Channel plays.
- The picture is never inside something that scrolls. The workspace is clipped, the watch screen's one scroll container is the description block, and the list is scrolled by assigning its `scrollTop`.
- The stage is its own stacking context. The band's button is raised above the lines it covers, and without that context it would also lie above a sheet's backdrop and stay live under an open sheet.
- Pocket has no transition, animation or keyframes. Every change of the picture's box is a jump.

## Dock latch

On Android the engine moves the native picture to the `<video>` rectangle only while a presentation is live. Paused, failed, recovering, suspending or stopping, the native view keeps its last rectangle in front of the page. A box that shrank then would leave the old picture covering the guide.

- `data-dock` sizes the picture and `data-mode` decides what is shown under it. They are separate attributes because they can differ.
- The dock follows the mode while the picture follows its box: where the page draws the picture (hosted and Linux), while there is no player, and while the player is starting, playing or waiting for a tap to start. In any other state the dock keeps the value it had. `nextDock` and `pictureFollows` in `app/src/features/stage/dock.ts` are the whole rule, and the shell derives the value during render.
- The latch stops the mode from changing the box. The window changes it too: the picture's height is taken from the window's, and the soft keyboard makes the window lower. So while the picture cannot follow, the shell also holds the box at the size it has. `usePicturePin` measures the box once, when following stops, and writes its width and height in pixels to the document root as `--pocket-pinned-w` and `--pocket-pinned-h`; it removes them when the picture follows again or the player goes away. Every rule in `pocket.css` that sizes the box, or places the guide or a sheet by it, reads those properties first. A paused picture with the guide and the keyboard open therefore keeps its rectangle, and the search field stays under it. Where the page draws the picture nothing is held.
- That gives four combinations. Watch and undocked is ordinary watching. Guide and docked is the ordinary guide. Guide and undocked, after opening the guide while paused or failed, keeps the picture across the window, hides the info block and puts the guide below. Watch and docked, after returning to the picture while it could not follow, keeps the picture band-sized with the rest of the watch screen under it. The picture takes its proper size as soon as it is live again.

## Controls and state

- In both layouts the player puts compact icon controls in the info block's slot. In pocket they are one row under the lower third: Pause or the recovery button, Mute and the Audio Track at the left; Full screen, the mpv switch, More and Stop at the right. The markup has one order and CSS orders the row by class.
- The Volume slider stays mounted and is shown where the row has room: in windows at least 30rem wide, and in short landscape from 56rem, where the row is 45% of the window. A phone has volume keys. The Audio Track gives way first when the row is short of room, and a narrow row hides it beside the Resume or Restart button.
- The More menu opens downward in pocket and is held to the room under the picture's bottom edge, which the player measures each time the menu opens. Where it does not fit below its button it opens beside the button instead, and it scrolls if even that room is too small. It never reaches the picture.
- While the player section is itself the fullscreen element, which is the case on Android, only its own subtree is drawn. The player then renders its controls inline as the bar, with Restart and Copy diagnostics as plain buttons instead of a More menu, whatever the shell asked for. `controlPresentation` in `app/src/features/stage/stage-chrome.ts` decides this, and the player tells it whether its section is the fullscreen element by asking the document, not by what the shell's fullscreen target implies. The installed Linux app has no target until its capability read answers, so its section can already be fullscreen when the target becomes the document root: the bar then stays in the section. The Full screen button likewise leaves whatever element is fullscreen (`fullscreenToggleTarget`), whatever the target is by then. The `<video>` is a sibling of the controls and does not remount on the switch. Android fullscreen is otherwise as it was.
- Outside fullscreen the player section in pocket is all picture. Its heading is clipped, not removed, and still announces the state.
- On Android the native surface covers the text the player draws in the picture. So in pocket the lower third words the state itself whenever it is not simply playing: "Tuning", "Paused", "Reconnecting", the name of a failure, or "No sound" for a picture the player knows to be playing without it. It takes the Channel Group's place in the line and is plain text.

## List guide

- The timeline grid is Theater's only. In pocket each guide row is a list row, 4.25rem tall: the Channel Number, the Channel's name, the live Programme's title, then the start time and title of what follows and the minutes left. A line along the row's bottom edge shows the Programme's progress. On the playing row the Channel Number, the title and that line are amber.
- A row with several Quality Variants has its quality chips at the right of the first line. The marking is the one ADR 0006 set: the row's own button carries `data-acceptance-channel` when the row is one Channel, and each chip does otherwise.
- A Channel with nothing on at the time is titled by its name. With no Programmes at all the row says "No guide data. The channel still plays."
- The first page is still 40 Channels with a "More channels" button, and rows are not virtualised.
- Entering guide mode centres the playing row. A change of Channel while the list is on screen moves the list only when the row is out of view. Choosing another Channel Group starts the list at its top.
- The guide's toolbar holds the search field and Sources. The lane holds the time chip, the Channel Group chips and Choose groups.

## Time chips

- A chip at the start of the Channel Group lane reads "Now" or the chosen time. It opens a row with Now and up to six of the next full hours on the local clock. Each is an instant at which the local clock reads a full hour, also across a change of the clocks, whatever its size: an hour the clocks skip is left out.
- With a time chosen, each row shows the Programme on at that time with its start and end, without progress. A row with nothing then says "Nothing listed at" and the time. Core keeps the earliest hundred Programmes of a Channel in a window, so a Channel with very short Programmes may have none for the later times; from the start of the last Programme kept the row says "The guide for this time is not loaded." and does not claim that nothing is listed. Pressing a row still tunes the Channel now.
- A chosen time counts as now once the clock reaches it.
- Until the viewer first opens the time row the guide reads the same three-hour window as the timeline. From then on, for as long as the app stays open, pocket reads eight hours. One wider read covers every chip, so choosing a time is a lookup in rows the client already holds and sends no request. While that wider read is in flight a row with nothing found at the chosen time says nothing, not that nothing is listed.
- A change of window keeps the list as long as it was. When the viewer has loaded more than the first page, the client reads the same number of pages for the new window before it shows any of them, so the rows are replaced in place and the list keeps its scroll position. A window the client already holds with fewer pages, such as Theater's three hours after more Channels were loaded in pocket's eight, is read on from the pages it has; the catalog cache never goes stale, so it would otherwise be shown short. This holds for the half-hourly move of the window too, in both layouts.

## Short landscape and wide windows

- A window at most 1050 px wide and 600 px tall in landscape, which is a phone on its side, shows both at once: the picture, the lower third and the controls in a left column, the list in a right column, 45 to 55. Modes and the dock change nothing visible there. The channel bar, the search button and the description block are hidden. While nothing plays the list has the whole width.
- The picture there takes the height the info block leaves, so nothing in that block may change height while the native picture cannot follow. When the window itself gets lower in that time, the held size keeps the picture's box and its column as they were, and the info block is cut at the window's edge instead. The control row is one line of a fixed height: it scrolls sideways where its controls do not fit, and the status messages come last in it instead of taking a line of their own.
- Wider pocket windows, such as a tablet or a narrow desktop window, have the same two modes. The list lays its rows out in columns of at least 21rem.
- The search field's results run the width of the window, at most 35rem, with each name over its detail.
- Search, Choose groups and Sources open as sheets under the picture: under the band, under the full-width picture when the latch holds it, or under the masthead when nothing plays. They span a narrow window and are a 36rem panel at the right edge of a wide one. A press on the band or on the picture while a sheet is open closes the sheet and changes nothing else. The sheets and the picture read their sizes from the same custom properties on the root, the held size included, so they cannot drift apart. In short landscape the sheets cover the right column.

## What stays a contract

- Everything ADR 0006 lists: control labels, the `.hosted-player*` and `.programme-guide*` class names, the `Tune ` label prefix and every `data-acceptance-*` attribute. There is exactly one marked button per Channel in Channel Catalog order, before and after a tune.
- A list row's button is named `Tune ` and the Channel, nothing more. What the Channel has on, what follows and the minutes left are the button's description (`aria-describedby`), so a screen reader hears them after the name. The channel bar's buttons carry the neighbour's live title the same way.
- The buttons pocket adds (the channel bar, the band's button, the search button, the time chips) carry no `data-acceptance-*` attribute and no label that starts with `Tune `.
- `app/scripts/installed-css-performance.test.ts` now also pins that every selector in `pocket.css` is scoped to the pocket layout, that the file has no transition, animation or keyframes, that the stage is a stacking context, that the control row of a short landscape window is one line of a fixed height, that every rule which sizes the picture's box or places something by it reads the held size, and that the load notice starts at the top of a box too low for it.
- No read, IPC command, Rust or Kotlin code changed. The eight-hour window is the existing guide-window read with a later end.

## Consequences

- The Android acceptance harnesses run in pocket. After their first tune they press guide buttons, and set a Volume slider, that CSS has taken out of the layout. That works because the elements stay mounted and the scripts act through the DOM.
- A phone does not show the Volume slider outside fullscreen. Mute is in the row and the slider is in the fullscreen bar.
- Opening the time row costs one wider read of each guide page the viewer has loaded and of the rows around the playing Channel. Cold start is not affected.
- A Channel next to a large hidden Channel Group costs up to eight further reads of a hundred rows when it is tuned, until the bar has a Channel to offer on that side. They are cached like every catalog read.
- In short landscape a status message, such as "Diagnostics copied", may lie past the end of the control row until the row is scrolled.
- A picture held at its size keeps that size until it plays again or is stopped. Turning the phone while paused leaves the box at the size it had in the other orientation, which is where the native picture still is.
- On the night the clocks go back an hour, two chips carry the same label: they are two instants an hour apart.
- The latch, the held size and the inline fullscreen bar are covered by tests with a fake client that reports `pictureOverlay: false`; the held size is measured from a box the test lays out, since jsdom lays out none. The native picture itself is only seen in the [Android mobile review](../debug/android-mobile-review.md).

## Rejected alternatives

- Keeping the timeline on a phone is rejected. A phone's width has room for a Channel's name or for a stretch of time, not both, and the grid scrolled in two directions under a fixed picture.
- Holding only the dock, and leaving the box's size to the window, is rejected: the keyboard would shrink the box of a paused picture and put the guide's toolbar under the rectangle the native picture keeps.
- Resizing the picture's box in every state is rejected: a paused or failed native picture would stay at its old rectangle, over the guide. Animating the box is rejected for the same reason and because ADR 0006 already rules it out.
- Unmounting the guide in watch mode, or a tree per mode, is rejected. The acceptance scripts press guide rows while a Channel plays, and a remount of the player restarts the picture.
- Compact controls in the shell's slot during Android fullscreen are rejected: the slot is outside the fullscreen element and would not be drawn.
- A More menu that opens upward is rejected in pocket because it would lie over the picture, where native video covers it. Letting it turn upward when there is no room below is rejected for the same reason: in short landscape the row is the last thing in its column.
- Wrapping the control row in short landscape, or giving its status messages a line, is rejected: the picture's box would change height on Pause or on a message, exactly when native video does not follow it. Keeping an empty line for the messages is rejected because it would take a sixth of the picture for a message that is rarely there.
- Reading 61 rows around the playing Channel in pocket, as Theater does, is rejected: it triples a read made on every tune on the phone and still ends at a hidden Channel Group of thirty Channels. Reading further only at the edge costs nothing until the viewer gets there.
- Putting what the Channel has on into a list row's label is rejected: the `Tune ` label is a contract the acceptance scripts match on.
- A request per time chip is rejected in favour of one wider window. Reading eight hours from the start is rejected because it would add to every cold start on Android for viewers who never open the times.
- `scrollIntoView` for bringing the playing row into view is rejected. It may also scroll clipped ancestors and drag the picture's box with them.

## Revisit gates

Reconsider the latch if the Android engine comes to move the native picture in every state, or if the picture stops being a native surface above the WebView. Reconsider the eight-hour window and the six chips if the wider read is measured to be slow on the phone or viewers want to look further ahead. Reconsider the hidden Volume slider if a pocket device without volume keys appears.
