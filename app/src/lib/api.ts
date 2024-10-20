import ky from "ky";

export const API_URL =
  import.meta.env.MODE === "development"
    ? "http://localhost:8000"
    : "https://tv.ponbac.xyz";

export const api = ky.create({
  prefixUrl: API_URL,
  retry: 0,
});

export function searchProgrammes(query: string) {
  return api.get("search", { searchParams: { q: query } }).json<SearchResult>();
}

export interface ProgrammeResult {
  channelName: string;
  programmeTitle: string;
  programmeDesc: string;
  start: string;
  stop: string;
}

export interface ChannelResult {
  channelName: string;
}

export interface SearchResult {
  programmes: ProgrammeResult[];
  channels: ChannelResult[];
}
