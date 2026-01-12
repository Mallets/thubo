import numpy as np
import pandas as pd
import seaborn as sns
import matplotlib.pyplot as plt
from matplotlib.ticker import EngFormatter

def expand_to_log10_bounds(ymin, ymax):
    lo = 10 ** np.floor(np.log10(ymin))
    hi = 10 ** np.ceil(np.log10(ymax))
    return lo, hi

def binary_size_label(x):
    if x <= 0:
        return "0"

    p = int(np.ceil(np.log2(x)))
    v = 2 ** p
    
    if v >= 2**40:
        return f"{v / 2**30:.0f} Ti"
    elif v >= 2**30:
        return f"{v / 2**30:.0f} Gi"
    elif v >= 2**20:
        return f"{v / 2**20:.0f} Mi"
    elif v >= 2**10:
        return f"{v / 2**10:.0f} Ki"
    else:
        return str(v)
    
def annotate_points(ax, x, y, formatter=str, offset=9, rotation=315):
    for xi, yi in zip(x, y):
        ax.annotate(
            formatter(yi),
            (xi, yi),
            textcoords="offset points",
            xytext=(-offset, offset),
            ha="center",
            rotation=rotation,
            fontsize=9
        )

csv_path = "/tmp/recv.log"

df = pd.read_csv(csv_path,
    header=None,
    names=["role", "size", "msg_per_s", "batch_per_s", "bytes_per_batch"]
)

# ---- Compute bit/s ----
df["bit_per_s"] = df["size"] * df["msg_per_s"] * 8

# ---- Median aggregation ----
agg = (
    df.groupby("size", as_index=False)
      .agg(
          msg_per_s=("msg_per_s", "median"),
          bit_per_s=("bit_per_s", "median")
      )
)

# Treat size as categorical
# agg["size"] = agg["size"].astype(str)
agg["size_label"] = agg["size"].apply(binary_size_label)

# ---- Plot ----
sns.set_theme(style="whitegrid")

fig, (ax1, ax2) = plt.subplots(
    2, 1,
    figsize=(12, 8),
    sharex=True
)

# ---- msg/s subplot ----
sns.lineplot(
    data=agg,
    x="size_label",
    y="msg_per_s",
    marker="o",
    errorbar="ci",
    ax=ax1
)

annotate_points(
    ax1,
    agg["size_label"],
    agg["msg_per_s"],
    formatter=lambda v: EngFormatter(places=1).format_eng(v),
)

ax1.set_title("Throughput over TCP (localhost) - Apple M4")

ax1.set_yscale("log")
ax1.set_ylabel("msg/s")
ax1.yaxis.set_major_formatter(EngFormatter())
ax1.grid(True, which="minor", axis="y", linestyle=":", linewidth=0.7, alpha=0.4)

ymin1, ymax1 = ax1.get_ylim()
lo1, hi1 = expand_to_log10_bounds(ymin1, ymax1)
ax1.set_ylim(lo1, hi1)

# ---- bit/s subplot ----
sns.lineplot(
    data=agg,
    x="size_label",
    y="bit_per_s",
    marker="o",
    errorbar="ci",
    ax=ax2
)

annotate_points(
    ax2,
    agg["size_label"],
    agg["bit_per_s"],
    formatter=lambda v: EngFormatter(places=1).format_eng(v),
)

ax2.set_yscale("log")
ax2.set_ylabel("bit/s")
ax2.set_xlabel("Message Size (bytes)")
ax2.yaxis.set_major_formatter(EngFormatter())
ax2.grid(True, which="minor", axis="y", linestyle=":", linewidth=0.7, alpha=0.4)

ymin2, ymax2 = ax2.get_ylim()
lo2, hi2 = expand_to_log10_bounds(ymin2, ymax2)
ax2.set_ylim(lo2, hi2)

plt.xticks(rotation=45)
plt.tight_layout()
plt.savefig(
    "throughput.svg",
    format="svg",
    bbox_inches="tight"
)
