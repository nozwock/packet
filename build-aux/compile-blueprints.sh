#!/usr/bin/env bash
#
# Compile blueprint files to UI files all in the same directory.
#
# Usage: ./compile-blueprints.sh PATH_TO_BLUEPRINT_COMPILER OUTPUT_DIR BASE_INPUT_DIR [INPUT_FILE…]
#
# See https://gitlab.gnome.org/World/fractal/-/blob/209da3c268691b51e15b736a86bf5ae4c24d13ba/build-aux/compile-blueprints.sh

set -e

compiler="$1"
shift
output_dir="$1"
shift
base_input_dir="$1"
shift

# For debugging.
# echo "Compiling files in $input_dir to $output_dir with $compiler"

for input_file in "$@"
do
    # Change extension.
    output_file="${input_file%.blp}.ui"
    # Remove base input dir to get relative path.
    output_file="${output_file#$base_input_dir\/}"
    # Replace slashes with dashes.
    output_file="$output_dir/${output_file//\//-}"

    # For debugging.
    # echo "Compiling $input_file to $output_file"

    "$compiler" compile --output "$output_file" "$input_file"
done
