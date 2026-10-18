require "json"

package = JSON.parse(File.read(File.join(__dir__, "package.json")))

Pod::Spec.new do |s|
  s.name         = "Checkgate"
  s.version      = package["version"]
  s.summary      = package["description"]
  s.homepage     = "https://github.com/checkgate-sh/checkgate"
  s.license      = package["license"]
  s.authors      = "ThinkGrid Labs"

  s.platforms    = { :ios => "13.4" }
  s.source       = { :git => "https://github.com/checkgate-sh/checkgate.git", :tag => "v#{s.version}" }

  # The native module (ios/) and the JSI host (cpp/). The Rust engine is not a
  # source file: a build phase compiles it (below) and the linker picks it up.
  s.source_files = "ios/**/*.{h,mm}", "cpp/**/*.{h,cpp}"
  s.preserve_paths = "Cargo.toml", "src/**/*", "rust-core/**/*", "scripts/build-ios-rust.sh"

  s.script_phase = {
    :name => "Build Checkgate Rust engine",
    :script => 'bash "${PODS_TARGET_SRCROOT}/scripts/build-ios-rust.sh"',
    :execution_position => :before_compile,
  }

  rust_link = {
    "LIBRARY_SEARCH_PATHS" => "\"${PODS_CONFIGURATION_BUILD_DIR}/Checkgate\"",
    "OTHER_LDFLAGS" => "-lcheckgate_rn",
  }
  s.pod_target_xcconfig = {
    "CLANG_CXX_LANGUAGE_STANDARD" => "c++20",
    "HEADER_SEARCH_PATHS" => "\"$(PODS_TARGET_SRCROOT)/cpp\"",
  }.merge(rust_link) # when the pod links as a dynamic framework
  s.user_target_xcconfig = rust_link # when it links statically into the app

  if respond_to?(:install_modules_dependencies, true)
    # React Native >= 0.71: React-Core, jsi, and the new-architecture deps.
    install_modules_dependencies(s)
  else
    s.dependency "React-Core"
  end
end
