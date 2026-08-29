# Sparrow TV

Sparrow TV presents an on-device channel catalog and plays a selected live channel on the owner's Linux or Android device.

## Language

**Source Configuration**:
The single local configuration containing one required M3U Source and one optional EPG Source.
_Avoid_: Provider config, playlist config, account

**M3U Source**:
The configured location of the playlist from which live Channels and Channel Groups are derived.
_Avoid_: Playlist URL, media source

**EPG Source**:
The optional configured location of schedule data used to enrich Channels with Programmes.
_Avoid_: Guide URL, XMLTV link

**Source Snapshot**:
The last validated copy of an M3U Source or EPG Source retained on the device so its contribution to the Channel Catalog remains available without network access.
_Avoid_: Cache entry, cached catalog, downloaded source

**Stale Source Snapshot**:
A Source Snapshot old enough to trigger a refresh attempt but still valid for building the Channel Catalog when refreshing fails.
_Avoid_: Expired snapshot, invalid snapshot

**Channel Catalog**:
The on-device collection of Channels and Channel Groups derived from the M3U Source and optionally enriched with Programmes from the EPG Source.
_Avoid_: Playlist, channel list, media library

**Channel**:
A playable live television entry in the Channel Catalog.
_Avoid_: Stream, station, media

**Channel Identifier**:
The opaque, Source Configuration-scoped identity of a Channel, retained across Source Snapshot refreshes while that Channel remains recognizable.
_Avoid_: Channel URL, tvg-id, stream ID

**Channel Group**:
A category from the M3U Source used to organize Channels in the Channel Catalog.
_Avoid_: Category, folder, bouquet

**Programme**:
Scheduled content associated with a Channel by the EPG Source.
_Avoid_: Show, event, media

**Playback Source**:
The ephemeral provider location and request information resolved from a Channel when starting a Playback Session.
_Avoid_: Channel URL, stream URL, media URL

**Playback Session**:
The period from selecting a channel for playback until that playback is stopped or replaced by another channel.
_Avoid_: Stream session, player instance

**Primary Playback Engine**:
The engine attempted first for every new Playback Session on a target device.
_Avoid_: Default player, main player, primary path

**Fallback Playback Engine**:
An alternative engine that may start only after the Primary Playback Engine has stopped or failed; it never runs concurrently for the same Playback Session.
_Avoid_: Backup player, secondary path

**Playback Failover**:
A user-authorized transition from a stopped or failed Primary Playback Engine to a Fallback Playback Engine within the same Playback Session.
_Avoid_: Automatic fallback, seamless failover

**Audio Track**:
A selectable audio rendition carried by a channel's Playback Source, described when available by language, label, and codec. Selecting an Audio Track is distinct from changing volume or mute state.
_Avoid_: Audio channel, sound stream, language setting

**Audio Track Preference**:
The last Audio Track selected for a Channel, reused for later Playback Sessions when that rendition remains available.
_Avoid_: Global language preference, default audio track
