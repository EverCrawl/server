# TODO: only update when pkt files change

echo "Checking for packetc"
$_ = packetc --help
if (!$?) { 
    echo "Installing packetc"
    cargo install --git https://github.com/EverCrawl/packetc.git
}

git remote update
git submodule update --recursive

echo "Updating schemas"
pushd schemas
git fetch
git checkout master
git pull
popd

echo "Deleting old schemas"
if (Test-Path src/schemas) {
    rm -r -fo src/schemas
}

echo "Compiling schemas"
packetc rust schemas src/schemas

# all the compiled schemas in src/schemas need to be re-exported:
# for each compiled schema
#   get filename without extension
#   append it as "pub mod {filename};" into a string
#   write that string to src/schemas/mod.rs
echo "Collecting compiled schemas"
$index = ""
Get-ChildItem -Path src/schemas -Filter *.rs -Recurse -File -Name | ForEach-Object {
    $index += "pub mod " + [System.IO.Path]::GetFileNameWithoutExtension($_) + ";`n"
}
echo "Writing src/schemas/mod.rs"
$index | Out-File -FilePath src/schemas/mod.rs

