#!/bin/sh
# Gentle check: the PC has no CPU fan, so use 2 jobs at low priority.
exec nice -n 19 cargo check -j 2 "$@"
