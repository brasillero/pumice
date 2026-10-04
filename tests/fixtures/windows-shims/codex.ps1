#!/usr/bin/env pwsh
$basedir=Split-Path $MyInvocation.MyCommand.Definition -Parent

$exe=""
if ($PSVersionTable.PSVersion -lt "6.0" -or $IsWindows) {
  $exe=".exe"
}
& "$basedir/node$exe" "$basedir/node_modules/@openai/codex/bin/codex.js" $args
exit $LASTEXITCODE
