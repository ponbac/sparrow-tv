use std::{env, fs};

use crate::parse::Playlist;

mod parse;

#[tokio::main]
async fn main() {
    dotenvy::from_filename(".env.local").expect("Failed to load .env file");
    dbg!(env::var("M3U_PATH").unwrap());

    let playlist_content =
        fs::read_to_string("playlist_full.m3u").expect("Failed to read playlist file");
    let playlist: Playlist = playlist_content.parse().expect("Failed to parse playlist");
    println!("{}", playlist.to_m3u());

    // let playlist_content = reqwest::get(env::var("M3U_PATH").unwrap())
    //     .await
    //     .expect("Failed to fetch playlist")
    //     .text()
    //     .await
    //     .expect("Failed to read playlist file");
    // let playlist: Playlist = playlist_content.parse().expect("Failed to parse playlist");
    // println!("{}", playlist.to_m3u());

    println!("Hello, world!");
}
