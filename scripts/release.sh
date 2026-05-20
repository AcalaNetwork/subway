#!/bin/bash
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to print colored output
print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

# Check if gh CLI is installed
if ! command -v gh &> /dev/null; then
    print_error "GitHub CLI (gh) is not installed. Please install it first."
    exit 1
fi

# Check if user is authenticated with gh
if ! gh auth status &> /dev/null; then
    print_error "Not authenticated with GitHub CLI. Please run 'gh auth login' first."
    exit 1
fi

# Parse command line arguments
VERSION=""
BUMP_TYPE=""
PRE_RELEASE=false

case "${1:-}" in
    --major|--minor|--patch)
        BUMP_TYPE="${1#--}"
        shift
        ;;
    --pre)
        PRE_RELEASE=true
        shift
        ;;
    -h|--help|"")
        echo "Usage: $0 [VERSION|--major|--minor|--patch] [--pre]"
        echo ""
        echo "Examples:"
        echo "  $0 1.2.3              # Release version 1.2.3"
        echo "  $0 --patch            # Bump patch version (0.1.0 -> 0.1.1)"
        echo "  $0 --minor            # Bump minor version (0.1.0 -> 0.2.0)"
        echo "  $0 --major            # Bump major version (0.1.0 -> 1.0.0)"
        echo "  $0 --patch --pre      # Bump and mark as pre-release"
        exit 0
        ;;
    *)
        VERSION="$1"
        shift
        ;;
esac

# Check for --pre flag after version/bump
if [ "${1:-}" = "--pre" ]; then
    PRE_RELEASE=true
fi

# Get current version from Cargo.toml
CURRENT_VERSION=$(grep '^version = ' Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
print_info "Current version: $CURRENT_VERSION"

# Determine new version
if [ -n "$VERSION" ]; then
    NEW_VERSION="$VERSION"
elif [ -n "$BUMP_TYPE" ]; then
    # Parse current version
    IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"

    case $BUMP_TYPE in
        major)
            NEW_VERSION="$((MAJOR + 1)).0.0"
            ;;
        minor)
            NEW_VERSION="${MAJOR}.$((MINOR + 1)).0"
            ;;
        patch)
            NEW_VERSION="${MAJOR}.${MINOR}.$((PATCH + 1))"
            ;;
        *)
            print_error "Invalid bump type: $BUMP_TYPE. Use 'major', 'minor', or 'patch'"
            exit 1
            ;;
    esac
else
    print_error "No version specified. Use: $0 <version|--major|--minor|--patch>"
    exit 1
fi

print_info "New version: $NEW_VERSION"

# Confirm with user
echo ""
read -p "Continue with release $NEW_VERSION? (y/N) " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    print_warning "Release cancelled"
    exit 0
fi

# Check if working directory is clean
if [ -n "$(git status --porcelain)" ]; then
    print_error "Working directory is not clean. Please commit or stash changes first."
    exit 1
fi

# Update version in Cargo.toml
print_info "Updating version in Cargo.toml..."
sed -i.bak "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" Cargo.toml
rm Cargo.toml.bak

# Verify the change
if ! grep -q "^version = \"$NEW_VERSION\"" Cargo.toml; then
    print_error "Failed to update version in Cargo.toml"
    exit 1
fi

# Commit the version bump
print_info "Committing version bump..."
git add Cargo.toml
git commit -m "Bump version to $NEW_VERSION"

# Create git tag
TAG_NAME="v$NEW_VERSION"
print_info "Creating git tag: $TAG_NAME"
git tag -a "$TAG_NAME" -m "Release $NEW_VERSION"

# Push to remote
print_info "Pushing to remote..."
git push
git push origin "$TAG_NAME"

# Create GitHub release
print_info "Creating GitHub release with auto-generated notes..."

# Create the release with auto-generated notes
if [ "$PRE_RELEASE" = true ]; then
    gh release create "$TAG_NAME" \
        --title "$TAG_NAME" \
        --generate-notes \
        --prerelease
else
    gh release create "$TAG_NAME" \
        --title "$TAG_NAME" \
        --generate-notes
fi

print_info "Release $NEW_VERSION created successfully!"
print_info "Release page: https://github.com/AcalaNetwork/subway/releases/tag/$TAG_NAME"
