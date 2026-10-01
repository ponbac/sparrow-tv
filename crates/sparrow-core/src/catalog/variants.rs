//! Quality Variant and Channel Number rules. They stay private to core:
//! adapters and clients only see the number, quality and base name they yield.

use crate::{domain::ChannelQuality, identity};

/// The Channel Number and picture quality stored for one catalogued Channel.
pub(super) struct Placement {
    pub(super) number: u32,
    pub(super) quality: Option<ChannelQuality>,
}

/// Places `(name, group)` pairs given in Channel Catalog order.
///
/// A Channel joins the family of the Channel immediately before it when both
/// are Quality Variants in one Channel Group with the same base name and the
/// family does not hold that quality yet. Every family takes the next number.
pub(super) fn placements<'a>(channels: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<Placement> {
    let mut number = 0_u32;
    let mut open: Option<OpenFamily<'a>> = None;

    channels
        .map(|(name, group)| {
            let variant = split(name)
                .map(|(quality, base)| (quality, identity::normalize_identity_field(&base)));
            let quality = variant.as_ref().map(|(quality, _)| *quality);
            match (&mut open, variant) {
                (Some(family), Some((quality, base)))
                    if family.group == group
                        && family.base == base
                        && family.qualities & quality_bit(quality) == 0 =>
                {
                    family.qualities |= quality_bit(quality);
                }
                (_, variant) => {
                    number = number.checked_add(1).expect(
                        "the bounded M3U payload cannot contain more than u32::MAX Channels",
                    );
                    open = variant.map(|(quality, base)| OpenFamily {
                        group,
                        base,
                        qualities: quality_bit(quality),
                    });
                }
            }
            Placement { number, quality }
        })
        .collect()
}

/// Splits a Channel name into its picture quality and base name.
///
/// The last whitespace-separated token naming a quality counts. The base name
/// keeps the other tokens in their original casing; a name that is nothing but
/// the token has no variant.
pub(super) fn split(name: &str) -> Option<(ChannelQuality, String)> {
    let (position, quality) = name
        .split_whitespace()
        .enumerate()
        .filter_map(|(position, token)| quality_token(token).map(|quality| (position, quality)))
        .last()?;

    let mut base = String::with_capacity(name.len());
    for (index, token) in name.split_whitespace().enumerate() {
        if index == position {
            continue;
        }
        if !base.is_empty() {
            base.push(' ');
        }
        base.push_str(token);
    }
    (!base.is_empty()).then_some((quality, base))
}

struct OpenFamily<'a> {
    group: &'a str,
    base: String,
    qualities: u8,
}

const fn quality_bit(quality: ChannelQuality) -> u8 {
    match quality {
        ChannelQuality::Sd => 1,
        ChannelQuality::Hd => 1 << 1,
        ChannelQuality::Fhd => 1 << 2,
        ChannelQuality::Uhd => 1 << 3,
    }
}

fn quality_token(token: &str) -> Option<ChannelQuality> {
    let token = token.trim_matches(|character: char| character.is_ascii_punctuation());
    [
        ("sd", ChannelQuality::Sd),
        ("hd", ChannelQuality::Hd),
        ("fhd", ChannelQuality::Fhd),
        ("uhd", ChannelQuality::Uhd),
        ("4k", ChannelQuality::Uhd),
    ]
    .into_iter()
    .find_map(|(text, quality)| token.eq_ignore_ascii_case(text).then_some(quality))
}

#[cfg(test)]
mod tests {
    use crate::domain::ChannelQuality::{self, Fhd, Hd, Sd, Uhd};

    use super::{placements, split};

    #[test]
    fn a_quality_token_is_one_whole_word_and_the_last_one_counts() {
        for (name, expected) in [
            ("SVT1 FHD SE", Some((Fhd, "SVT1 SE"))),
            ("Cinema One HD", Some((Hd, "Cinema One"))),
            ("sd Regional", Some((Sd, "Regional"))),
            ("Arena (uhd)", Some((Uhd, "Arena"))),
            ("Arena [HD]:", Some((Hd, "Arena"))),
            ("Nature 4K", Some((Uhd, "Nature"))),
            ("Sport HD Extra FHD", Some((Fhd, "Sport HD Extra"))),
            ("HD", None),
            ("(4k)", None),
            ("HDNet Movies", None),
            ("SDTV Classics", None),
            ("World News", None),
            ("", None),
        ] {
            assert_eq!(
                split(name),
                expected.map(|(quality, base)| (quality, base.to_owned())),
                "{name:?}"
            );
        }
    }

    #[test]
    fn adjacent_variants_of_one_base_name_share_a_channel_number() {
        assert_eq!(
            placed(&[
                ("World News", "News"),
                ("SVT1 SD SE", "Sweden"),
                ("svt1  HD se", "Sweden"),
                ("SVT1 FHD SE", "Sweden"),
                ("SVT2 HD SE", "Sweden"),
                ("Cinema One", "Films"),
            ]),
            [
                (1, None),
                (2, Some(Sd)),
                (2, Some(Hd)),
                (2, Some(Fhd)),
                (3, Some(Hd)),
                (4, None),
            ]
        );
    }

    #[test]
    fn a_repeated_quality_starts_a_new_family() {
        assert_eq!(
            placed(&[
                ("Arena HD", "Sport"),
                ("Arena FHD", "Sport"),
                ("Arena HD", "Sport"),
                ("Arena SD", "Sport"),
            ]),
            [(1, Some(Hd)), (1, Some(Fhd)), (2, Some(Hd)), (2, Some(Sd)),]
        );
    }

    #[test]
    fn variants_in_different_channel_groups_never_merge() {
        assert_eq!(
            placed(&[("Arena SD", "Sport"), ("Arena HD", "Sport Extra")]),
            [(1, Some(Sd)), (2, Some(Hd))]
        );
    }

    #[test]
    fn variants_separated_by_another_channel_never_merge() {
        assert_eq!(
            placed(&[
                ("Arena SD", "Sport"),
                ("Arena", "Sport"),
                ("Arena HD", "Sport"),
                ("Stadium FHD", "Sport"),
                ("Arena FHD", "Sport"),
            ]),
            [
                (1, Some(Sd)),
                (2, None),
                (3, Some(Hd)),
                (4, Some(Fhd)),
                (5, Some(Fhd)),
            ]
        );
    }

    fn placed(channels: &[(&str, &str)]) -> Vec<(u32, Option<ChannelQuality>)> {
        placements(channels.iter().copied())
            .into_iter()
            .map(|placement| (placement.number, placement.quality))
            .collect()
    }
}
