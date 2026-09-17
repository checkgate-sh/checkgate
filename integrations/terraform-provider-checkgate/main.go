// terraform-provider-checkgate manages Checkgate feature flags and segments as
// code with Terraform or OpenTofu.
package main

import (
	"context"
	"flag"
	"log"

	"github.com/hashicorp/terraform-plugin-framework/providerserver"

	"github.com/checkgate-sh/terraform-provider-checkgate/internal/provider"
)

// version is set at release time via -ldflags "-X main.version=...".
var version = "dev"

func main() {
	var debug bool
	flag.BoolVar(&debug, "debug", false, "run with support for debuggers like delve")
	flag.Parse()

	err := providerserver.Serve(context.Background(), provider.New(version), providerserver.ServeOpts{
		Address: "registry.terraform.io/checkgate-sh/checkgate",
		Debug:   debug,
	})
	if err != nil {
		log.Fatal(err.Error())
	}
}
