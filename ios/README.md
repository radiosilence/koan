# koan iOS

Native iOS client for koan. Connects to `koan graphql` over HTTP.

## Requirements

- iOS 17+
- Xcode 15.4+ / Swift 5.10+
- A running koan instance with `koan graphql` serving on the network

## Build

```bash
cd ios
swift build
```

Or open `Package.swift` in Xcode and build the `Koan` target.

## Architecture

Swift Package (no .xcodeproj). Zero third-party dependencies — just Foundation and SwiftUI.

- **GraphQLClient** — lightweight URLSession-based client with typed responses
- **Models** — Codable structs matching the koan GraphQL schema
- **Views** — SwiftUI with NavigationStack, async/await data loading

## Setup

1. Run `koan graphql` on your Mac (defaults to `http://localhost:3694`)
2. Launch the app
3. Go to Settings tab, enter the server URL, tap "Test Connection"
4. Browse your library in the Library tab

## What's wired up

- Artist list with search
- Album list per artist
- Track list per album (tap to replace queue + play)
- Server URL configuration with connection test
- Pull-to-refresh on all lists
