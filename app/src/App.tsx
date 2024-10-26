import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { searchProgrammes } from "./lib/api";
import { Input } from "./components/ui/input";
import { Search, Clock, Calendar, Tv } from "lucide-react";
import { Button } from "./components/ui/button";
import { Card, CardContent } from "./components/ui/card";
import { Badge } from "./components/ui/badge";
import { Switch } from "./components/ui/switch";
import { Label } from "./components/ui/label";

function App() {
  const [searchQuery, setSearchQuery] = useState("");
  const [includeHidden, setIncludeHidden] = useState(false);

  const { data } = useQuery({
    queryKey: ["programmes", searchQuery, includeHidden],
    queryFn: () => searchProgrammes(searchQuery, includeHidden),
    enabled: !!searchQuery && searchQuery.length > 3,
  });

  const formatTime = (dateString: string) => {
    return new Date(dateString).toLocaleTimeString("en-GB", {
      hour: "2-digit",
      minute: "2-digit",
    });
  };

  console.log(data);

  return (
    <div className="min-h-screen bg-gradient-to-br from-purple-400 via-pink-500 to-red-500 p-4">
      <div className="container mx-auto">
        <h1 className="text-4xl font-bold mb-6 text-center text-white drop-shadow-lg">
          📺 TV Programme Search
        </h1>
        <div className="flex mb-6 justify-center">
          <Input
            type="text"
            placeholder="Search programmes..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="mr-2 w-full max-w-md bg-white/90 backdrop-blur-sm"
          />
          <Button
            onClick={() => {}}
            className="bg-yellow-400 text-black hover:bg-yellow-500"
          >
            <Search className="mr-2 h-4 w-4" /> Search
          </Button>
        </div>
        <div className="flex items-center space-x-2 flex-row-reverse gap-2">
          <Switch
            id="include-hidden"
            checked={includeHidden}
            onCheckedChange={setIncludeHidden}
          />
          <Label htmlFor="include-hidden" className="text-white font-semibold">
            Include hidden channels
          </Label>
        </div>
        <div className="flex flex-col gap-8">
          <div>
            <h2 className="text-2xl font-semibold mb-4 text-white">Channels</h2>
            <div className="flex flex-wrap gap-2">
              {data?.channels.map((channel, index) => (
                <Badge
                  key={index}
                  variant="secondary"
                  className="bg-white/80 text-purple-700"
                >
                  <Tv className="mr-1 h-4 w-4" />
                  {channel.channelName}
                </Badge>
              ))}
            </div>
          </div>
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-6">
            {data?.programmes.map((programme, index) => (
              <Card
                key={index}
                className="overflow-hidden bg-white/90 backdrop-blur-sm hover:shadow-lg transition-shadow duration-300"
              >
                <div className="bg-gradient-to-r from-purple-600 to-pink-600 p-4">
                  <h3 className="text-lg font-bold text-white leading-tight">
                    {programme.programmeTitle}
                  </h3>
                </div>
                <CardContent className="p-4">
                  <div className="flex justify-between items-center mb-2">
                    <Badge
                      variant="outline"
                      className="bg-purple-100 text-purple-800 border-purple-300"
                    >
                      {programme.channelName}
                      {programme.channelGroup
                        ? ` (${programme.channelGroup})`
                        : ""}
                    </Badge>
                    <div className="flex items-center text-sm text-gray-500">
                      <Clock className="mr-1 h-4 w-4" />
                      {formatTime(programme.start)} -{" "}
                      {formatTime(programme.stop)}
                    </div>
                  </div>
                  <p className="text-sm text-gray-600 mb-2">
                    {programme.programmeDesc}
                  </p>
                  <div className="flex items-center text-xs text-gray-400">
                    <Calendar className="mr-1 h-3 w-3" />
                    {new Date(programme.start).toLocaleDateString("en-GB", {
                      day: "2-digit",
                      month: "2-digit",
                      year: "numeric",
                    })}
                  </div>
                </CardContent>
              </Card>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

export default App;
