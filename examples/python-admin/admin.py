"""Example: LVQR admin client in Python.

Usage:
    pip install lvqr
    python admin.py
"""

from lvqr import LvqrClient

def main():
    client = LvqrClient("http://localhost:8080")

    # Health check
    if not client.healthz():
        print("LVQR relay is not running")
        return

    print("LVQR relay is healthy")

    # Stats
    stats = client.stats()
    print(f"Tracks: {stats.tracks}")
    print(f"Subscribers: {stats.subscribers}")

    # List streams
    streams = client.list_streams()
    if streams:
        print(f"\nActive streams ({len(streams)}):")
        for stream in streams:
            print(f"  {stream.name} ({stream.subscribers} viewers)")
    else:
        print("\nNo active streams")

if __name__ == "__main__":
    main()
