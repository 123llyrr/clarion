#!/usr/bin/env python3
"""Retired GTK global-menu exporter installer.

Clarion now leaves application menus inside each application. Existing exporter
files and release backups are retained for running clients and rollback; this
entry point never installs, removes or changes an active session environment.
"""
import argparse


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", help=argparse.SUPPRESS)
    parser.add_argument("--prefix", help=argparse.SUPPRESS)
    parser.parse_args()
    parser.exit(
        2,
        "GTK global-menu integration is retired. Existing module files are preserved; "
        "Clarion uses native application menus.\n",
    )


if __name__ == "__main__":
    main()
