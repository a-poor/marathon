# Run with `brew ruby`, against a generated cask and its extracted native binary.
# Exercise Homebrew's actual hook ordering without installing into its prefix or
# relying on Gatekeeper being enabled on the machine running the test.
require "cask/cask_loader"
require "fileutils"
require "open3"
require "tmpdir"

cask = Cask::CaskLoader::FromContentLoader.new(File.read(ARGV.fetch(0))).load(config: nil)
source_binary = Pathname(ARGV.fetch(1)).realpath
quarantine = "com.apple.quarantine"

Dir.mktmpdir("marathon-quarantine-smoke-") do |directory|
  staged = Pathname(directory)
  binary = staged/"marathon"
  FileUtils.cp(source_binary, binary)
  system("/usr/bin/xattr", "-w", quarantine, "0081;#{Time.now.to_i.to_s(16)};MarathonSmoke;", binary.to_s,
         exception: true)
  attributes, status = Open3.capture2("/usr/bin/xattr", binary.to_s)
  unless status.success? && attributes.lines.map(&:strip).include?(quarantine)
    raise "could not quarantine the disposable binary"
  end

  # Redirect only the staging directory. Keep the generated hooks unchanged and
  # let Homebrew's ArtifactSet decide when they run relative to completions.
  cask.define_singleton_method(:staged_path) { staged }
  checked = false
  cask.artifacts.each do |artifact|
    if artifact.is_a?(Cask::Artifact::GeneratedCompletion)
      attributes, status = Open3.capture2("/usr/bin/xattr", binary.to_s)
      raise "could not inspect the staged binary" unless status.success?
      if attributes.lines.map(&:strip).include?(quarantine)
        raise "Marathon is still quarantined when Homebrew reaches completion generation"
      end
      checked = true
      break
    end
    # Linking binaries and writing completions belong to the full install test.
    artifact.install_phase if artifact.is_a?(Cask::Artifact::AbstractFlightBlock)
  end
  raise "generated cask has no completion-generation artifact" unless checked

  %w[bash zsh fish].each do |shell|
    output, error, status = Open3.capture3(binary.to_s, "completions", shell)
    unless status.success? && output.include?("marathon")
      raise "failed to generate #{shell} completions: #{error}"
    end
  end
end

puts "PASS: quarantine is cleared before Homebrew reaches completion generation."
