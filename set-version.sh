#!/usr/bin/env bash
set -e

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored messages
error() {
    echo -e "${RED}Error: $1${NC}" >&2
}

success() {
    echo -e "${GREEN}$1${NC}"
}

info() {
    echo -e "${YELLOW}$1${NC}"
}

# Check if version argument is provided
if [ $# -ne 1 ]; then
    error "Usage: $0 <version>"
    echo "Example: $0 1.2.3"
    exit 1
fi

VERSION="$1"

# Validate semantic versioning format (e.g., 1.2.3 or 0.1.0)
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    error "Invalid version format: $VERSION"
    echo "Version must follow semantic versioning: MAJOR.MINOR.PATCH (e.g., 1.2.3)"
    exit 1
fi

# Check if we're in a git repository
if ! git rev-parse --git-dir > /dev/null 2>&1; then
    error "Not in a git repository"
    exit 1
fi

# Check if working directory is clean
if ! git diff-index --quiet HEAD --; then
    error "Working directory is not clean. Commit or stash your changes first."
    git status --short
    exit 1
fi

# Get current version from Cargo.toml
CURRENT_VERSION=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
info "Current version: $CURRENT_VERSION"
info "New version: $VERSION"

# Check if tag already exists
if git rev-parse "v$VERSION" >/dev/null 2>&1; then
    error "Tag v$VERSION already exists"
    exit 1
fi

# Update version in Cargo.toml
echo ""
info "Updating Cargo.toml..."
sed -i "s/^version = \".*\"/version = \"$VERSION\"/" Cargo.toml

# Update Cargo.lock by building
info "Updating Cargo.lock..."
cargo build --release > /dev/null 2>&1

# Show the diff
echo ""
info "Changes to be committed:"
git diff Cargo.toml Cargo.lock

# Commit the changes
echo ""
info "Creating commit..."
git add Cargo.toml Cargo.lock
git commit -m "Bump version to $VERSION"

# Create git tag
info "Creating git tag v$VERSION..."
git tag -a "v$VERSION" -m "Release version $VERSION"

# Success message
echo ""
success "✓ Version bumped to $VERSION"
success "✓ Commit created"
success "✓ Git tag v$VERSION created"

# Show next steps
echo ""
info "Next steps:"
echo "  1. Review the changes:"
echo "     git show HEAD"
echo ""
echo "  2. Push the commit and tag:"
echo "     git push origin main"
echo "     git push origin v$VERSION"
echo ""
echo "  3. Or push both at once:"
echo "     git push origin main --follow-tags"
