const fs = require('fs');
const path = require('path');

const rootDir = path.resolve(__dirname, '../../..');
const sdkDir = path.resolve(__dirname, '..');
const coreDir = path.join(rootDir, 'core');
const destDir = path.join(sdkDir, 'rust-core');

console.log(`[Checkgate] Bundling Rust core from ${coreDir}...`);

if (!fs.existsSync(destDir)) {
    fs.mkdirSync(destDir, { recursive: true });
}

function copyRecursive(src, dest) {
    const stats = fs.statSync(src);
    if (stats.isDirectory()) {
        if (!fs.existsSync(dest)) fs.mkdirSync(dest);
        fs.readdirSync(src).forEach(child => {
            copyRecursive(path.join(src, child), path.join(dest, child));
        });
    } else {
        fs.copyFileSync(src, dest);
    }
}

// Copy core/src and core/Cargo.toml
copyRecursive(path.join(coreDir, 'src'), path.join(destDir, 'src'));
fs.copyFileSync(path.join(coreDir, 'Cargo.toml'), path.join(destDir, 'Cargo.toml'));

// Both crates inherit version, license and lints from the monorepo workspace,
// which is not part of the npm package. The app build (Gradle / the CocoaPods
// build phase) compiles the crate from node_modules, so resolve those fields
// here and make the package its own workspace root.
const rootCargo = fs.readFileSync(path.join(rootDir, 'Cargo.toml'), 'utf8');
const workspacePackage = rootCargo.split('[workspace.package]')[1] || '';
function workspaceField(name) {
    const match = new RegExp(`^\\s*${name}\\s*=\\s*"([^"]+)"`, 'm').exec(workspacePackage);
    if (!match) throw new Error(`[Checkgate] workspace.package.${name} not found in Cargo.toml`);
    return match[1];
}
const version = workspaceField('version');
const license = workspaceField('license');

function standaloneManifest(toml) {
    return toml
        .replace(/^version\.workspace\s*=\s*true\s*$/m, `version = "${version}"`)
        .replace(/^license\.workspace\s*=\s*true\s*$/m, `license = "${license}"`)
        .replace(/^\[lints\]\s*\nworkspace\s*=\s*true\s*$/m, '');
}

const coreCargoPath = path.join(destDir, 'Cargo.toml');
fs.writeFileSync(coreCargoPath, standaloneManifest(fs.readFileSync(coreCargoPath, 'utf8')));

// Point the SDK crate at the bundled core for the npm release.
const sdkCargoPath = path.join(sdkDir, 'Cargo.toml');
let sdkCargo = standaloneManifest(fs.readFileSync(sdkCargoPath, 'utf8'));
sdkCargo = sdkCargo.replace(/path\s*=\s*"\.\.\/\.\.\/core"/, 'path = "./rust-core"');
if (!/^\[workspace\]/m.test(sdkCargo)) sdkCargo += '\n[workspace]\n';
fs.writeFileSync(sdkCargoPath, sdkCargo);

console.log('[Checkgate] Rust core bundled and Cargo.toml patched for React Native SDK.');
