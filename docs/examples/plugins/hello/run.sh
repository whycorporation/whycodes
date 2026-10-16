#!/bin/sh
# Example shell plugin. Tool arguments arrive as PLUGIN_ARG_<KEY>;
# PLUGIN_WORKSPACE is the session's working directory.
echo "hello ${PLUGIN_ARG_INPUT:-world} (from $PLUGIN_WORKSPACE)"
