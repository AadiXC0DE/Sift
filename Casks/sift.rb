cask "sift" do
  version "1.1.1"
  sha256 "d8ca81fa45beca9df8ecd506ac0991f12570664c87768ec2306ea5dbdb36ece6"

  url "https://github.com/AadiXC0DE/Sift/releases/download/v#{version}/Sift_#{version}_universal.dmg",
      verified: "github.com/AadiXC0DE/Sift/"
  name "Sift"
  desc "Keyboard-first Gmail client for macOS"
  homepage "https://usesift.xyz"

  depends_on macos: :ventura

  app "Sift.app"

  caveats <<~EOS
    One tiny macOS detour: Sift is not notarized by Apple yet.
    Homebrew verifies the download, but macOS may still show a warning.
    First-launch steps, including when Open Anyway is missing:
      https://usesift.xyz/download#first-launch
  EOS
end
