#!/bin/bash

# Check if version argument is provided
if [ $# -ne 1 ]; then
    echo "Error: Version argument is required"
    echo "Usage: ./deploy.sh <version>"
    echo "Example: ./deploy.sh 1.0.0"
    exit 1
fi

# Validate semver format (basic validation)
if ! [[ $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Error: Version must be in semver format (x.y.z)"
    echo "Example: 1.0.0"
    exit 1
fi

VERSION=$1
IMAGE_NAME="ponbac/sparrow:$VERSION"

# Update version in Cargo.toml
echo "Updating version in Cargo.toml to $VERSION"
sed -i "s/^version = \".*\"/version = \"$VERSION\"/" Cargo.toml

if [ $? -ne 0 ]; then
    echo "Error: Failed to update version in Cargo.toml"
    exit 1
fi

echo "Building Docker image: $IMAGE_NAME"
docker build -t $IMAGE_NAME .

if [ $? -eq 0 ]; then
    echo "Successfully built Docker image"
    echo "Pushing Docker image to registry..."
    docker push $IMAGE_NAME
    
    if [ $? -eq 0 ]; then
        echo "Successfully pushed Docker image: $IMAGE_NAME"
    else
        echo "Error: Failed to push Docker image"
        exit 1
    fi
else
    echo "Error: Docker build failed"
    exit 1
fi

