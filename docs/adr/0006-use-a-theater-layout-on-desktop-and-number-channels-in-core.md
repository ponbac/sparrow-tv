# Use a Theater layout on desktop and number Channels in core

On desktop the picture fills the window and the info, controls and guide are drawn over it or beside it: the Theater layout. Android and small windows keep the stacked layout, with the picture on top, the info below it and the guide below that. In both layouts every guide row carries a Channel Number, and adjacent picture-quality copies of a Channel are one row with a quality switch. This replaces the Split Stage shell.

## Layout

- The layout is Theater when the page may draw over the picture and either the window is at least 1051 px wide and 601 px tall or the document root is fullscreen. Otherwise it is stacked.
- Whether the page may draw over the picture is the `pictureOverlay` capability. It is true for hosted and installed Linux and false on Android, where Media3 draws a native surface above the WebView. The installed app is stacked until its capability read answers.
- Theater has two modes. In watch mode the picture fills the window; the playing Channel's info, the controls and a rail of nearby Channels lie over its lower edge. That chrome and the top bar hide after three seconds without input while a picture plays in the page, and return on pointer or key input. In guide mode the picture docks top-left, the info sits beside it and the guide fills the rest.
- Tuning a Channel enters watch mode. Guide mode is forced while nothing plays and while the picture is in the external mpv window.
- Keys work in Theater only: G opens and closes the guide, `/` goes to search, F toggles fullscreen, the up and down arrows change Channel in watch mode, and Escape returns from the guide to the picture. An arrow press shows its target at once and tunes it 350 ms after the last press. The stacked layout keeps only F, on the focused player.
- Choosing a Channel, a Programme cell or a search result tunes it. There is no separate selection or preview: the info block describes what is on now on the playing Channel.
- Both layouts are one React tree. Layout, mode and hidden chrome are attributes on the shell (`data-layout`, `data-mode`, `data-chrome`) and docking is CSS only, so a change of layout or mode never remounts the player or its `<video>`. In Theater the player portals its controls into the info block.
- Nothing over the picture is blurred, the picture docks without animation, and Theater rules fade opacity only. The installed Linux app composites in software, where blur and per-frame video scaling are unmeasured. `app/scripts/installed-css-performance.test.ts` pins these rules.

## Fullscreen

This amends the fullscreen bullet of [ADR 0001](0001-shared-native-http-playback.md). Where the page may draw over the picture, the fullscreen element is the document root, not the player. Fullscreen then outlasts the player: it stays across Stop and Channel changes, and entering it from a small window switches that window to the Theater layout. On Android the player remains the fullscreen element, with its controls over the picture as before. The button, double-click and F toggle it in both cases. With external mpv the button still toggles the mpv window.

## Channel Numbers and Quality Variants

- `sparrow-core` decides both once, when it builds the Channel Catalog. Every Channel read model carries `number` and `variant` (`{ quality, baseName }` or null); adapters and clients never derive them.
- A Channel Number is the 1-based position of the Channel's guide row in Channel Catalog order, across all Channel Groups. It comes from position alone, not from a number the M3U Source may carry, and it changes when the source reorders.
- A Quality Variant is a Channel whose name has a whitespace-separated token equal to SD, HD, FHD, UHD or 4K once ASCII punctuation is trimmed and case is folded. The last such token counts and 4K means UHD. The base name is the name without that token.
- A Channel joins the family of the Channel immediately before it only when both are Quality Variants in one Channel Group with the same normalised base name and the family does not hold that quality yet. Members of a family share one Channel Number. Copies that are not adjacent, or that sit in another Channel Group, stay separate rows.
- Each Quality Variant stays its own Channel with its own Channel Identifier, Playback Source, EPG match and Audio Track Preference. `name` is never rewritten: identity, search, EPG matching and Agent Control keep using it.
- The client folds adjacent rows with one Channel Number into one guide row. It plays the highest quality unless the viewer picked another for that family; the pick is kept in the browser's or WebView's local storage on that device.

## Reads for the playing Channel

Two existing reads gained an optional input; no read was added.

- `schedule` takes `from`, an instant, and returns the Channel's Programmes that end after it, in start order, paged. A bad `from` is the `schedule-from` input error.
- The guide-window read takes `around`, a Channel Identifier, and starts the page half a page before that Channel in Channel Catalog order. It is rejected together with a group or a cursor. The page's `next` is the ordinary all-groups cursor.

The info block reads the schedule from the guide window's start. It also uses the rows around the playing Channel, as do the rail and the arrow keys in Theater: 61 rows in Theater, 9 in stacked. Both reads are keyed on the catalog generation like every other catalog query.

## Consequences

- The Android acceptance harness and the Linux lab probe match on markup and button text. Control labels, the `.hosted-player*` and `.programme-guide*` class names, the `Tune ` label prefix and every `data-acceptance-*` attribute are therefore a contract. A row with several Quality Variants marks each quality chip with `data-acceptance-channel` instead of the row, so there is still one marked button per Channel.
- Compact Theater controls are icon buttons that keep their label in the DOM. Stop and the mpv switch are always mounted; Restart and Copy diagnostics sit in a More menu.
- A lone Channel with a quality token is still a Quality Variant. Its row is titled by the base name and has no switch.
- The arrow keys stop at the edge of the loaded rows around the playing Channel, and they skip Channel Groups the viewer has hidden.

## Rejected alternatives

- Overlaying the picture on Android is rejected: the native surface covers anything the page draws there.
- A separate React tree per layout, or moving the player between parents, is rejected because the remount would restart the picture.
- Blur behind the overlay and an animated dock are rejected until they are measured on the installed Linux app.
- Numbering Channels or deciding families in the client is rejected. The client holds pages, not the Channel Catalog, so it cannot know a position; and the family rule belongs with the other identity rules in core ([ADR 0003](0003-share-one-core-across-sibling-adapters.md)).
- Merging Quality Variants into one Channel is rejected because each has its own Playback Source, EPG match and Audio Track Preference.
- A new read for the rows around a Channel is rejected in favour of one optional input on the guide-window read, which already has the projection, caps and cursor.

## Revisit gates

Reconsider the no-blur and no-animation rule if installed Linux composition is measured with them. Reconsider adjacency if the M3U Source lists quality copies apart from each other, and the token list if it names qualities another way. Reconsider the Android layout if its picture stops being a native surface above the WebView.
