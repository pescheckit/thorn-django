FROM rust:1.85-bookworm

# Install Python 3.11 dev headers, pip, venv, and pkg-config so that
# the pyo3 build script can locate libpython via pkg-config.
RUN apt-get update && apt-get install -y \
    python3 \
    python3-pip \
    python3-dev \
    python3-venv \
    libpython3-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Create a venv so that pip installs are isolated from the system.
RUN python3 -m venv /venv
ENV PATH="/venv/bin:$PATH"
ENV VIRTUAL_ENV=/venv

# Install Django, DRF, and the pyo3 build helper.
# The pyo3-build-config package is a pure-Python utility — it is NOT
# the Rust crate, but it lets us verify the interpreter at image build
# time and is a useful sanity-check dependency.
RUN pip install --no-cache-dir \
    "django>=4.2,<6.0" \
    "djangorestframework>=3.14" \
    pyo3-build-config

# Tell pyo3 which Python interpreter to link against.
# This must point to the venv interpreter so that the .so produced by
# cargo links against the same libpython that Django runs under.
ENV PYO3_PYTHON=/venv/bin/python3

# Copy the entire thorn workspace into the image.
# The docker-compose build context is the repository root (..), so every
# path below is relative to /home/bram/work/thorn.
WORKDIR /app
COPY . /app/

# Add the thorn-django Python package to the venv so that
# `python -m thorn_django` works without any extra PYTHONPATH.
RUN pip install --no-cache-dir -e /app/thorn-django/python

# Build the thorn binary.  The workspace lives at /app/thorn, and the
# thorn-django Rust crate is a workspace dependency declared via a
# relative path so cargo resolves it automatically.
RUN cd /app/thorn && cargo build --release

# Expose the thorn binary on PATH so the test script can call it by name.
ENV PATH="/app/thorn/target/release:$PATH"

# Default command: run the full test suite.
CMD ["/app/thorn-django/test.sh"]
