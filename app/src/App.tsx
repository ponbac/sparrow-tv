import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { API_URL, ProgrammeResult, searchProgrammes } from "./lib/api";
import { Input } from "./components/ui/input";
import { Search, Clock, Calendar, Tv } from "lucide-react";
import { Button } from "./components/ui/button";
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
} from "./components/ui/card";
import { Badge } from "./components/ui/badge";
import { Switch } from "./components/ui/switch";
import { Label } from "./components/ui/label";
import { TvPlayer } from "./components/tv-player";

export default function App() {
  const [searchQuery, setSearchQuery] = useState("");
  const [includeHidden, setIncludeHidden] = useState(false);
  const [selectedUrl, setSelectedUrl] = useState<string | null>(null);

  const { data } = useQuery({
    queryKey: ["programmes", searchQuery, includeHidden],
    queryFn: () => searchProgrammes(searchQuery, includeHidden),
    enabled: !!searchQuery && searchQuery.length > 3,
  });

  const randomName = ["Tojvi", "Kjelle", "Ralph"][
    Math.floor(Math.random() * 3)
  ];

  return (
    <div className="min-h-screen bg-background p-4">
      <div className="container mx-auto">
        <h1 className="text-4xl font-bold mb-6 text-center text-foreground drop-shadow-lg">
          📺 {randomName} TV
        </h1>

        <SearchBar searchQuery={searchQuery} onSearchChange={setSearchQuery} />

        <HiddenChannelsToggle
          includeHidden={includeHidden}
          onToggleHidden={setIncludeHidden}
        />

        <div className="flex flex-col gap-8">
          {data?.channels && (
            <ChannelList
              channels={data.channels}
              onChannelSelect={setSelectedUrl}
            />
          )}

          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
            {data?.programmes.map((programme, index) => (
              <ProgrammeCard
                key={index}
                programme={programme}
                onSelect={setSelectedUrl}
              />
            ))}
          </div>
        </div>
      </div>

      {selectedUrl && (
        <TvPlayer
          url={`${API_URL}/proxy/${selectedUrl}`}
          onClose={() => setSelectedUrl(null)}
        />
      )}
    </div>
  );
}

const SearchBar = (props: {
  searchQuery: string;
  onSearchChange: (query: string) => void;
}) => {
  return (
    <div className="flex mb-6 justify-center items-center">
      <Input
        type="text"
        placeholder="Search programmes..."
        value={props.searchQuery}
        onChange={(e) => props.onSearchChange(e.target.value)}
        className="mr-2 w-full max-w-md bg-background/90 backdrop-blur-sm rounded-xl h-10"
        autoFocus
      />
      <Button
        onClick={() => {}}
        className="bg-primary text-primary-foreground hover:bg-primary/90"
      >
        <Search className="mr-2 h-4 w-4" /> Search
      </Button>
    </div>
  );
};

const HiddenChannelsToggle = (props: {
  includeHidden: boolean;
  onToggleHidden: (value: boolean) => void;
}) => {
  return (
    <div className="flex items-center space-x-2 flex-row-reverse gap-2">
      <Switch
        id="include-hidden"
        checked={props.includeHidden}
        onCheckedChange={props.onToggleHidden}
      />
      <Label htmlFor="include-hidden" className="text-foreground font-semibold">
        Include hidden channels
      </Label>
    </div>
  );
};

const ChannelList = (props: {
  channels: Array<{ channelName: string; url: string | null }>;
  onChannelSelect: (url: string) => void;
}) => {
  return (
    <div>
      <h2 className="text-2xl font-semibold mb-4 text-foreground">Channels</h2>
      <div className="flex flex-wrap gap-2">
        {props.channels?.map((channel, index) => (
          <Badge
            key={index}
            variant="secondary"
            className="bg-secondary text-secondary-foreground cursor-pointer hover:bg-secondary/70 rounded-xl flex items-center justify-center gap-1 hover:text-primary transition-colors"
            onClick={() => channel.url && props.onChannelSelect(channel.url)}
          >
            <Tv className="mr-1 size-4" />
            <span className="text-sm">{channel.channelName}</span>
          </Badge>
        ))}
      </div>
    </div>
  );
};

const ProgrammeCard = (props: {
  programme: ProgrammeResult;
  onSelect: (url: string) => void;
}) => {
  return (
    <Card
      className="overflow-hidden bg-card/90 backdrop-blur-sm hover:border-primary border-2 transition-colors duration-300 cursor-pointer"
      onClick={() =>
        props.programme.channelUrl && props.onSelect(props.programme.channelUrl)
      }
    >
      <CardHeader className="bg-primary p-4">
        <div className="flex justify-between items-center">
          <h3 className="text-lg font-bold text-primary-foreground leading-tight">
            {props.programme.programmeTitle}
          </h3>
          <div className="flex items-center text-sm text-primary-foreground">
            <Clock className="mr-1 size-4" />
            {formatTime(props.programme.start)} -{" "}
            {formatTime(props.programme.stop)}
          </div>
        </div>
      </CardHeader>
      <CardContent className="p-4 flex flex-col">
        <Badge
          variant="secondary"
          className="flex items-center justify-between gap-1.5 px-3 py-1.5 text-sm font-medium bg-secondary/50 text-secondary-foreground hover:bg-secondary/70 transition-colors w-full mb-2 rounded-xl"
        >
          {props.programme.channelName}
          {props.programme.channelGroup && (
            <span className="text-xs text-muted-foreground">
              {props.programme.channelGroup}
            </span>
          )}
        </Badge>
        <p className="text-sm text-muted-foreground mb-2">
          {props.programme.programmeDesc}
        </p>
      </CardContent>
      <CardFooter>
        <div className="ml-auto flex items-center text-xs text-muted-foreground">
          <Calendar className="mr-1 h-3 w-3" />
          {new Date(props.programme.start).toLocaleDateString("en-GB", {
            day: "2-digit",
            month: "2-digit",
            year: "numeric",
          })}
        </div>
      </CardFooter>
    </Card>
  );
};

function formatTime(dateString: string) {
  return new Date(dateString).toLocaleTimeString("en-GB", {
    hour: "2-digit",
    minute: "2-digit",
  });
}
