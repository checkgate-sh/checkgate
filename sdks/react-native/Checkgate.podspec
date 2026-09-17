require "json"

package = JSON.parse(File.read(File.join(__dir__, "package.json")))

Pod::Spec.new do |s|
  s.name         = "Checkgate"
  s.version      = package["version"]
  s.summary      = package["description"]
  s.homepage     = "https://github.com/checkgate-sh/checkgate"
  s.license      = package["license"]
  s.authors      = "ThinkGrid Labs"

  s.platforms    = { :ios => "13.0" }
  s.source       = { :git => "https://github.com/checkgate-sh/checkgate.git", :tag => "v#{s.version}" }

  # Include JS, C++, and Rust source
  s.source_files = "cpp/*.{h,cpp}", "src/**/*.{rs}", "rust-core/**/*.{rs,toml}"
  s.preserve_paths = "Cargo.toml", "src/**/*", "rust-core/**/*"

  # JSI dependencies
  s.dependency "React-Core"
  
  # Ensure the C++ compiler can find JSI headers
  s.pod_target_xcconfig = {
    "CLANG_CXX_LANGUAGE_STANDARD" => "c++17",
    "HEADER_SEARCH_PATHS" => "\"$(PODS_TARGET_SRCROOT)/cpp\""
  }
end
